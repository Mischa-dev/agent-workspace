import type { ChangeSetView } from '../api/generated/operations'

// Recent-edit outlines (C1.10): every write to a page, sheet or board is a
// change set naming what it touched. The editors outline those items, with
// who changed them and an Undo, as changes land and, on the "Recent edits"
// toggle, for the last day.

/** The kinds of thing that keep changes. */
export type ChangeKind = 'page' | 'sheet' | 'whiteboard'

/** How long a change counts as just landed, and is outlined unasked. */
export const LANDED_MS = 2 * 60 * 1000

/** One outlined item: the newest change that touched it. */
export type Outline = {
  changeId: string
  actor: string
  actorId: string
  at: string
  undoable: boolean
}

/** A sheet cell's key in an outline map: tab, row and column (from 1). */
export function cellKey(tabId: string, row: number, column: number) {
  return `${tabId}:${row}:${column}`
}

/** The items a change touched, as keys: block ids, cell keys or element ids. */
export function touchedItems(kind: ChangeKind, touched: unknown): string[] {
  if (!touched || typeof touched !== 'object') return []
  const record = touched as Record<string, unknown>
  if (kind === 'sheet') {
    const cells = Array.isArray(record.cells) ? record.cells : []
    return cells.flatMap((cell) => {
      if (!cell || typeof cell !== 'object') return []
      const { tabId, row, column } = cell as Record<string, unknown>
      return typeof tabId === 'string' && typeof row === 'number' && typeof column === 'number' ? [cellKey(tabId, row, column)] : []
    })
  }
  const list = kind === 'page' ? record.blocks : record.elements
  return Array.isArray(list) ? list.filter((item): item is string => typeof item === 'string') : []
}

/** Whether a change counts: not undone, and not itself an undo already undone. */
function live(change: ChangeSetView) {
  return !change.undoneBy
}

/**
 * The outlined items: the newest live change for each. With `all` off, only
 * changes that landed in the last two minutes from someone else; with it
 * on, every change listed (the last day's).
 */
export function outlines(kind: ChangeKind, changes: ChangeSetView[], options: { viewerId: string; all: boolean; now: number }) {
  const byItem = new Map<string, Outline>()
  // Newest first, so the first change to name an item wins.
  const sorted = [...changes].sort((a, b) => b.updatedAt.localeCompare(a.updatedAt))
  for (const change of sorted) {
    if (!live(change)) continue
    if (!options.all) {
      if (change.actorId === options.viewerId) continue
      if (options.now - Date.parse(change.updatedAt) > LANDED_MS) continue
    }
    const outline: Outline = {
      changeId: change.id,
      actor: change.actor ?? 'Someone',
      actorId: change.actorId,
      at: change.updatedAt,
      undoable: change.undoable,
    }
    for (const item of touchedItems(kind, change.touched)) if (!byItem.has(item)) byItem.set(item, outline)
  }
  return byItem
}

/** The change a bar offers to undo: the newest outlined one. */
export function newestOutlined(changes: ChangeSetView[], shown: Map<string, Outline>) {
  const ids = new Set([...shown.values()].map((outline) => outline.changeId))
  return [...changes].sort((a, b) => b.updatedAt.localeCompare(a.updatedAt)).find((change) => ids.has(change.id)) ?? null
}

/** How many items a change touched, in words: "3 cells". */
export function describeTouched(kind: ChangeKind, change: ChangeSetView) {
  const count = touchedItems(kind, change.touched).length + (more(change.touched) ?? 0)
  const noun = kind === 'page' ? 'block' : kind === 'sheet' ? 'cell' : 'element'
  return `${count} ${noun}${count === 1 ? '' : 's'}`
}

function more(touched: unknown) {
  if (!touched || typeof touched !== 'object') return 0
  const value = (touched as Record<string, unknown>).more
  return typeof value === 'number' ? value : 0
}
