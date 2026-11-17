import type { CommentThreadView } from '../api/generated/operations'
import { bounds, cellKey, type Selection } from './sheetModel'

/** A sheet anchor as the server keeps it: rows and columns from 0. */
export type SheetAnchor =
  | { kind: 'cell'; tabId: string; row: number; column: number }
  | { kind: 'range'; tabId: string; from: [number, number]; to: [number, number] }

function sheetAnchor(anchor: unknown): SheetAnchor | null {
  if (!anchor || typeof anchor !== 'object') return null
  const value = anchor as Record<string, unknown>
  if (typeof value.tabId !== 'string') return null
  if (value.kind === 'cell' && typeof value.row === 'number' && typeof value.column === 'number') {
    return { kind: 'cell', tabId: value.tabId, row: value.row, column: value.column }
  }
  const pair = (entry: unknown) =>
    Array.isArray(entry) && entry.length === 2 && entry.every((part) => typeof part === 'number') ? (entry as [number, number]) : null
  const from = pair(value.from)
  const to = pair(value.to)
  if (value.kind === 'range' && from && to) return { kind: 'range', tabId: value.tabId, from, to }
  return null
}

/**
 * The cells of one tab that open a comment thread, keyed like the grid's
 * cells (rows and columns from 1): a cell anchor marks its cell, a range
 * marks its top-left cell. Resolved threads leave no mark.
 */
export function commentedCells(threads: CommentThreadView[], tabId: string, withResolved = false) {
  const marks = new Map<string, CommentThreadView>()
  for (const thread of threads) {
    if (!withResolved && thread.status.toLowerCase() === 'resolved') continue
    const anchor = sheetAnchor(thread.anchor)
    if (!anchor || anchor.tabId !== tabId) continue
    const [row, column] = anchor.kind === 'cell' ? [anchor.row, anchor.column] : anchor.from
    const key = cellKey(row + 1, column + 1)
    if (!marks.has(key)) marks.set(key, thread)
  }
  return marks
}

/** The anchor for a new thread about what's selected: one cell, or a range. */
export function selectionAnchor(selection: Selection, tabId: string): SheetAnchor {
  const box = bounds(selection)
  if (box.top === box.bottom && box.left === box.right) {
    return { kind: 'cell', tabId, row: box.top - 1, column: box.left - 1 }
  }
  return { kind: 'range', tabId, from: [box.top - 1, box.left - 1], to: [box.bottom - 1, box.right - 1] }
}

/**
 * The thread that starts at a cell (from 1), open ones first, so the panel
 * finds a thread it just started, or one resolved while it was open.
 */
export function threadAt(threads: CommentThreadView[], tabId: string, row: number, column: number) {
  const key = cellKey(row, column)
  return commentedCells(threads, tabId).get(key) ?? commentedCells(threads, tabId, true).get(key) ?? null
}
