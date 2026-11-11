import type { QueryClient, QueryKey } from '@tanstack/react-query'

import { field, uuid, type RealtimeEnvelope } from './realtimeModel'

/**
 * Whiteboards and pages (AWD-53) and collections (AWD-90) that changed, from
 * the realtime socket. An open board or page listens for its own id and
 * fetches what changed since its last look; lists, a page's details and
 * history, and a collection's records refetch through the query cache. The
 * socket loads this with the first such event, so the app shell doesn't
 * carry it.
 */
export type WhiteboardChange = { whiteboardId: string; sceneSeq: number; version: number }
export type PageChange = { pageId: string; version: number }
export type CollectionChange = { collectionId: string }

/** Lists refetch a stroke at a time at most. */
const BOARD_LIST_DELAY_MS = 40
/** A page's details and the page list refetch every few seconds at most while someone types. */
const PAGE_DETAILS_DELAY_MS = 3_000
/** A collection's open lists refetch once for a burst of changes, such as an agent adding records one by one. */
const COLLECTION_DELAY_MS = 300

export function whiteboardChange(event: Pick<RealtimeEnvelope, 'type' | 'payload'>): WhiteboardChange | null {
  if (event.type !== 'whiteboard.updated') return null
  const whiteboardId = uuid(field(event.payload, 'whiteboardId', 'whiteboard_id'))
  const sceneSeq = field(event.payload, 'sceneSeq', 'scene_seq')
  const version = event.payload.version
  if (!whiteboardId || !Number.isSafeInteger(sceneSeq) || (sceneSeq as number) < 0 || !Number.isSafeInteger(version)) {
    return null
  }
  return { whiteboardId, sceneSeq: sceneSeq as number, version: version as number }
}

export function pageChange(event: Pick<RealtimeEnvelope, 'type' | 'payload'>): PageChange | null {
  if (event.type !== 'page.updated') return null
  const pageId = uuid(field(event.payload, 'pageId', 'page_id'))
  const version = event.payload.version
  if (!pageId || !Number.isSafeInteger(version) || (version as number) < 1) return null
  return { pageId, version: version as number }
}

export function collectionChange(event: Pick<RealtimeEnvelope, 'type' | 'payload'>): CollectionChange | null {
  if (event.type !== 'collection.updated') return null
  const collectionId = uuid(field(event.payload, 'collectionId', 'collection_id'))
  return collectionId ? { collectionId } : null
}

type Listeners<T> = Map<string, Set<(change: T) => void>>

const boards: Listeners<WhiteboardChange> = new Map()
const pages: Listeners<PageChange> = new Map()

function listen<T>(listeners: Listeners<T>, id: string, listener: (change: T) => void) {
  const set = listeners.get(id) ?? new Set()
  set.add(listener)
  listeners.set(id, set)
  return () => {
    set.delete(listener)
    if (set.size === 0) listeners.delete(id)
  }
}

export function onWhiteboardChange(whiteboardId: string, listener: (change: WhiteboardChange) => void) {
  return listen(boards, whiteboardId, listener)
}

export function onPageChange(pageId: string, listener: (change: PageChange) => void) {
  return listen(pages, pageId, listener)
}

/** The newest version heard for each page, so a page that opens just after a change still hears of it. */
const heard = new Map<string, number>()
const HEARD_PAGES = 500

export function lastHeardVersion(pageId: string) {
  return heard.get(pageId) ?? 0
}

function remember(change: PageChange) {
  const version = Math.max(change.version, lastHeardVersion(change.pageId))
  // Newest last, so the oldest goes first when there are too many.
  heard.delete(change.pageId)
  heard.set(change.pageId, version)
  if (heard.size > HEARD_PAGES) heard.delete(heard.keys().next().value as string)
}

const timers = new Map<string, number>()

function later(key: string, delay: number, run: () => void) {
  if (timers.has(key)) return
  timers.set(
    key,
    window.setTimeout(() => {
      timers.delete(key)
      run()
    }, delay),
  )
}

const isPageQuery = (projectId: string) => (query: { queryKey: QueryKey }) => {
  const [scope, id, area, kind] = query.queryKey
  return scope === 'project' && id === projectId && area === 'memory' && (kind === 'pages' || kind === 'page')
}

/**
 * A whiteboard.updated, page.updated or collection.updated event: tells
 * whoever has the board or page open, then refreshes the lists. For a
 * collection, everything open of it refetches (its records, views and fields,
 * and a record beside them) and so does the project's list of collections,
 * which counts records.
 */
export function documentChanged(queryClient: QueryClient, event: RealtimeEnvelope) {
  const board = whiteboardChange(event)
  if (board) {
    for (const listener of boards.get(board.whiteboardId) ?? []) listener(board)
    later('whiteboards', BOARD_LIST_DELAY_MS, () => void queryClient.invalidateQueries({ queryKey: ['whiteboards'] }))
    return
  }
  const collection = collectionChange(event)
  if (collection) {
    later(`collection:${event.projectId}:${collection.collectionId}`, COLLECTION_DELAY_MS, () => {
      void queryClient.invalidateQueries({ queryKey: ['project', event.projectId, 'collections', collection.collectionId] })
      void queryClient.invalidateQueries({ queryKey: ['project', event.projectId, 'collections', 'list'] })
    })
    return
  }
  const page = pageChange(event)
  if (page) {
    remember(page)
    for (const listener of pages.get(page.pageId) ?? []) listener(page)
    later(`pages:${event.projectId}`, PAGE_DETAILS_DELAY_MS, () =>
      void queryClient.invalidateQueries({ predicate: isPageQuery(event.projectId) }),
    )
  }
}
