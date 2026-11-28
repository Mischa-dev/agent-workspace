import type { DocEntry } from '../api/generated/operations'
import { spaceHref } from '../app/href'
import { projectAreaHref } from '../projects/locations'
import type { IconName } from '../ui/icons'
import { whiteboardPath } from '../whiteboards/api'

// The Docs tree: every page, sheet, whiteboard, collection and dashboard of
// a project in one list, with folders. The server sends a flat list (each
// entry names what holds it); this turns it into rows to draw.

export type DocKind = DocEntry['kind']

/** The kinds a person makes from Docs, in the order the New menu offers them. */
export const MADE_KINDS = ['page', 'sheet', 'whiteboard', 'collection', 'dashboard'] as const
export type MadeKind = (typeof MADE_KINDS)[number]

const KINDS: Record<DocKind, { label: string; icon: IconName }> = {
  folder: { label: 'Folder', icon: 'folder' },
  page: { label: 'Page', icon: 'list' },
  sheet: { label: 'Sheet', icon: 'table' },
  whiteboard: { label: 'Whiteboard', icon: 'whiteboard' },
  collection: { label: 'Collection', icon: 'layers' },
  dashboard: { label: 'Dashboard', icon: 'chart' },
}

export const kindLabel = (kind: DocKind) => KINDS[kind].label
export const kindIcon = (kind: DocKind) => KINDS[kind].icon

/** Where a document opens. A folder opens in place, so it has no address. */
export function docHref(projectId: string, entry: Pick<DocEntry, 'id' | 'kind'>): string | null {
  switch (entry.kind) {
    case 'folder':
      return null
    case 'page':
      return spaceHref(projectId, `/pages/${encodeURIComponent(entry.id)}`)
    case 'sheet':
      return projectAreaHref(projectId, 'sheets', entry.id)
    case 'collection':
      return projectAreaHref(projectId, 'collections', entry.id)
    case 'dashboard':
      return projectAreaHref(projectId, 'dashboards', entry.id)
    case 'whiteboard':
      return whiteboardPath(entry.id)
  }
}

export type DocRow = {
  entry: DocEntry
  depth: number
  /** How many things it holds directly; only folders and pages hold any. */
  holds: number
  open: boolean
}

const byTitle = (left: DocEntry, right: DocEntry) => {
  // Folders first, then everything by name, the way a file list reads.
  if ((left.kind === 'folder') !== (right.kind === 'folder')) return left.kind === 'folder' ? -1 : 1
  return left.title.localeCompare(right.title, undefined, { numeric: true, sensitivity: 'base' })
}

/** The rows to draw, top to bottom: each open folder's contents follow it. */
export function docRows(entries: readonly DocEntry[], opened: ReadonlySet<string>): DocRow[] {
  const known = new Set(entries.map((entry) => entry.id))
  const inside = new Map<string | null, DocEntry[]>()
  for (const entry of entries) {
    // Something whose holder isn't in the list is shown at the top, never lost.
    const parent = entry.parentId && known.has(entry.parentId) && entry.parentId !== entry.id ? entry.parentId : null
    const list = inside.get(parent)
    if (list) list.push(entry)
    else inside.set(parent, [entry])
  }
  const rows: DocRow[] = []
  const walk = (parent: string | null, depth: number, seen: Set<string>) => {
    for (const entry of [...(inside.get(parent) ?? [])].sort(byTitle)) {
      if (seen.has(entry.id)) continue
      seen.add(entry.id)
      const holds = inside.get(entry.id)?.length ?? 0
      const open = holds > 0 && opened.has(entry.id)
      rows.push({ entry, depth, holds, open })
      if (open) walk(entry.id, depth + 1, seen)
    }
  }
  walk(null, 0, new Set())
  return rows
}

/** A folder to put something in, with the folders above it in its name. */
export type FolderChoice = { id: string; label: string }

/** Every folder, as "Design / Sketches", leaving out `moving` and whatever it holds. */
export function folderChoices(entries: readonly DocEntry[], moving?: string): FolderChoice[] {
  const folders = new Map(entries.filter((entry) => entry.kind === 'folder').map((entry) => [entry.id, entry]))
  const path = (entry: DocEntry): string[] | null => {
    const names: string[] = []
    let at: DocEntry | undefined = entry
    for (let step = 0; at && step < 32; step += 1) {
      if (at.id === moving) return null
      names.unshift(at.title)
      at = at.parentId ? folders.get(at.parentId) : undefined
    }
    return names
  }
  return [...folders.values()]
    .flatMap((folder) => {
      const names = path(folder)
      return names ? [{ id: folder.id, label: names.join(' / ') }] : []
    })
    .sort((left, right) => left.label.localeCompare(right.label, undefined, { numeric: true, sensitivity: 'base' }))
}

/** Query keys of Docs' own reads (the operations key themselves). */
export const docKeys = {
  people: (projectId: string) => ['doc-access-people', projectId] as const,
}

const OPENED = 'aw.docs.open.'

/** The folders a person left open in this project, kept on this device. */
export function readOpened(projectId: string): Set<string> {
  try {
    const stored: unknown = JSON.parse(window.localStorage.getItem(OPENED + projectId) ?? '[]')
    return new Set(Array.isArray(stored) ? stored.filter((id): id is string => typeof id === 'string') : [])
  } catch {
    return new Set()
  }
}

export function writeOpened(projectId: string, opened: ReadonlySet<string>) {
  try {
    window.localStorage.setItem(OPENED + projectId, JSON.stringify([...opened].slice(-200)))
  } catch {
    // A full or blocked store only forgets which folders were open.
  }
}
