// Offline reading (W1.31): the one offline mechanism. When a person turns
// on "Read offline" and their workspace allows it, what they recently opened
// (the thing store, and the first page of recent conversations, lists and
// pages) is kept in IndexedDB for their account, and shown at start before
// the network answers. It's off by default, capped, kept per account, and
// cleared at sign-out, when the server says the session is gone, and when the
// person or the workspace turns it off. The service worker still never keeps
// API answers.

import type { QueryClient } from '@tanstack/react-query'

import { things as thingStore, type Thing } from './things'
import { forgetOffline, OFFLINE_DATABASE, offlineOwner, setOfflineOwner } from './offlinePointer'

/** How much is kept: things, conversations (with their latest messages), lists, and what pages need around them. */
export const OFFLINE_CAPS = { things: 500, conversations: 30, messages: 100, lists: 20, pages: 120, shell: 60 } as const

export type OfflineClass = 'conversation' | 'list' | 'page' | 'shell'

const at = (key: readonly unknown[], index: number) => (typeof key[index] === 'string' ? (key[index] as string) : undefined)

/**
 * Which cached answers are kept for reading offline, by their query key:
 * what a page shows (a conversation's messages, a list's first page, one
 * task, page, sheet, record or board) and what the shell needs to draw it.
 * Anything else (settings, members' details, tokens, search) never is.
 */
export function offlineClass(key: readonly unknown[]): OfflineClass | null {
  const root = at(key, 0)
  switch (root) {
    case 'session':
    case 'direct':
    case 'statuses':
    case 'custom-emoji':
      return 'shell'
    case 'projects':
      return at(key, 1) === 'rail' ? 'shell' : null
    case 'organization': {
      const part = at(key, 2)
      if (part === 'layout' || part === 'modules' || part === 'pages-space') return 'shell'
      return part === 'settings' && at(key, 3) === 'effective' ? 'shell' : null
    }
    case 'inbox':
    case 'my-work':
    case 'home':
    case 'later':
      return 'list'
    case 'things':
      return key.length === 3 ? 'page' : null
    case 'whiteboards':
      return at(key, 1) === 'list' ? 'list' : at(key, 1) === 'detail' ? 'page' : null
    case 'whiteboard-scene':
      return 'page'
    case 'project':
      return projectClass(key)
    default:
      return null
  }
}

function projectClass(key: readonly unknown[]): OfflineClass | null {
  const area = at(key, 2)
  const part = at(key, 3)
  if (key.length === 2) return 'shell'
  switch (area) {
    case 'navigation':
    case 'conversation-sidebar':
    case 'layout':
    case 'members':
      return 'shell'
    case 'channel-list':
      return 'list'
    case 'conversation':
      if (at(key, 4) === 'timeline') return key.length === 6 ? 'conversation' : 'page'
      return ['details', 'tabs', 'members', 'tasks'].includes(at(key, 4) ?? '') ? 'page' : null
    case 'work-items':
      if (part === 'workflow' || part === 'labels' || part === 'milestones') return 'shell'
      if (part === 'list' || part === 'board' || part === 'groups' || part === 'pages') return 'list'
      return (part === 'detail' || part === 'timeline') && key.length === 5 ? 'page' : null
    case 'memory':
      if (part === 'pages') return 'list'
      return part === 'page' && (key.length === 5 || at(key, 5) === 'discussion') ? 'page' : null
    case 'sheets':
      return part === 'list' ? 'list' : part ? 'page' : null
    case 'collections':
      if (part === 'list') return 'list'
      if (at(key, 4) === 'records') return at(key, 5) === 'pick' ? null : 'list'
      return at(key, 4) === 'record' || key.length === 4 ? 'page' : null
    default:
      return null
  }
}

const CLASS_CAP: Record<OfflineClass, number> = {
  conversation: OFFLINE_CAPS.conversations,
  list: OFFLINE_CAPS.lists,
  page: OFFLINE_CAPS.pages,
  shell: OFFLINE_CAPS.shell,
}

/** An answer as kept: a paged list keeps its first page, a conversation its latest messages. */
function trimmed(data: unknown, kind: OfflineClass): unknown {
  if (!data || typeof data !== 'object') return data
  const record = data as Record<string, unknown>
  if (Array.isArray(record.pages) && Array.isArray(record.pageParams)) {
    return { ...record, pages: record.pages.slice(0, 1), pageParams: record.pageParams.slice(0, 1) }
  }
  if (kind === 'conversation' && Array.isArray(record.messages) && record.messages.length > OFFLINE_CAPS.messages) {
    return { ...record, messages: record.messages.slice(-OFFLINE_CAPS.messages) }
  }
  return data
}

/** One kept answer: its key, what it said, and when. */
export type KeptQuery = { queryKey: readonly unknown[]; data: unknown; updatedAt: number }

/** Every answer in the cache of a kind worth keeping. */
export function cachedForOffline(queryClient: QueryClient): KeptQuery[] {
  return queryClient
    .getQueryCache()
    .getAll()
    .filter((query) => query.state.status === 'success' && offlineClass(query.queryKey) !== null)
    .map((query) => ({ queryKey: query.queryKey, data: query.state.data, updatedAt: query.state.dataUpdatedAt }))
}

/** The answers worth keeping, the most recent of each class up to its cap. */
export function pickForOffline(queries: readonly KeptQuery[]): KeptQuery[] {
  const counts: Record<OfflineClass, number> = { conversation: 0, list: 0, page: 0, shell: 0 }
  return [...queries]
    .sort((left, right) => right.updatedAt - left.updatedAt)
    .flatMap((query) => {
      const kind = offlineClass(query.queryKey)
      if (!kind) return []
      // The session opens everything else: kept whatever the cap.
      const session = at(query.queryKey, 0) === 'session'
      if (!session && counts[kind] >= CLASS_CAP[kind]) return []
      if (!session) counts[kind] += 1
      return [{ ...query, data: trimmed(query.data, kind) }]
    })
}

// --- Where it's kept ---------------------------------------------------------

export type OfflineCopy = { owner: string; savedAt: number; queries: KeptQuery[]; things: Thing[] }

type Store = {
  get(owner: string): Promise<OfflineCopy | undefined>
  put(copy: OfflineCopy): Promise<void>
  clear(): Promise<void>
}

const STORE = 'copies'

function memoryStore(): Store {
  const kept = new Map<string, OfflineCopy>()
  return {
    get: async (owner) => kept.get(owner),
    put: async (copy) => {
      kept.set(copy.owner, copy)
    },
    clear: async () => kept.clear(),
  }
}

function indexedStore(factory: IDBFactory): Store {
  let database: Promise<IDBDatabase> | null = null
  const open = () => {
    database ??= new Promise((resolve, reject) => {
      const opening = factory.open(OFFLINE_DATABASE, 1)
      opening.onupgradeneeded = () => {
        if (!opening.result.objectStoreNames.contains(STORE)) opening.result.createObjectStore(STORE, { keyPath: 'owner' })
      }
      opening.onsuccess = () => {
        // Signing out deletes the database (`forgetOffline`): let it go, and open afresh next time.
        opening.result.onversionchange = () => {
          opening.result.close()
          database = null
        }
        resolve(opening.result)
      }
      opening.onerror = () => {
        database = null
        reject(opening.error)
      }
    })
    return database
  }
  const run = async <T>(mode: IDBTransactionMode, work: (store: IDBObjectStore) => IDBRequest<T>) => {
    const db = await open()
    return new Promise<T>((resolve, reject) => {
      const pending = work(db.transaction(STORE, mode).objectStore(STORE))
      pending.onsuccess = () => resolve(pending.result)
      pending.onerror = () => reject(pending.error)
    })
  }
  return {
    get: (owner) => run('readonly', (store) => store.get(owner) as IDBRequest<OfflineCopy | undefined>),
    put: (copy) => run('readwrite', (store) => store.put(copy)).then(() => undefined),
    clear: () => run('readwrite', (store) => store.clear()).then(() => undefined),
  }
}

let store: Store = typeof indexedDB === 'undefined' ? memoryStore() : indexedStore(indexedDB)
/** Goes up on every clear, so a save that started before it never lands after it. */
let generation = 0

/** For tests: start from an empty store kept in memory. */
export function resetOfflineForTests() {
  store = memoryStore()
  generation += 1
  setOfflineOwner(null)
}

// --- Keeping, showing and forgetting ----------------------------------------

/** Write what's worth keeping now, for `owner`. */
export async function saveOffline(queryClient: QueryClient, owner: string): Promise<void> {
  const started = generation
  const queries = pickForOffline(cachedForOffline(queryClient))
  // Through JSON, so nothing that can't be stored (a function, a class) stops the write.
  const copy = JSON.parse(
    JSON.stringify({ owner, savedAt: Date.now(), queries, things: thingStore.snapshot(OFFLINE_CAPS.things) }),
  ) as OfflineCopy
  if (started !== generation || offlineOwner() !== owner) return
  await store.put(copy)
}

/**
 * Show the kept copy before the network answers, if this device keeps one.
 * Fresher answers already in the cache win. Says whether there was one.
 */
export async function restoreOffline(queryClient: QueryClient): Promise<boolean> {
  const owner = offlineOwner()
  if (!owner) return false
  const started = generation
  const copy = await store.get(owner).catch(() => undefined)
  // Cleared while it was read (signed out, someone else signed in): show none of it.
  if (!copy || copy.owner !== owner || started !== generation || offlineOwner() !== owner) return false
  for (const kept of copy.queries) {
    // A fresher answer already here wins.
    if ((queryClient.getQueryState(kept.queryKey)?.dataUpdatedAt ?? 0) >= kept.updatedAt) continue
    queryClient.setQueryData(kept.queryKey, kept.data, { updatedAt: kept.updatedAt })
  }
  for (const thing of copy.things) thingStore.put(thing)
  return true
}

/**
 * Keep `owner`'s copy current while reading offline is on: shortly after
 * answers arrive, and when the page is hidden. Returns the way to stop.
 */
export function keepOffline(queryClient: QueryClient, owner: string, delay = 2000): () => void {
  if (offlineOwner() !== owner) {
    // Another account's copy is never shown to this one.
    if (offlineOwner()) void clearOffline()
    setOfflineOwner(owner)
  }
  let timer: ReturnType<typeof setTimeout> | null = null
  const save = () => {
    if (timer) clearTimeout(timer)
    timer = null
    void saveOffline(queryClient, owner).catch(() => undefined)
  }
  const soon = () => {
    if (!timer) timer = setTimeout(save, delay)
  }
  const unsubscribe = queryClient.getQueryCache().subscribe((event) => {
    if (event.type === 'updated' && event.action.type === 'success' && offlineClass(event.query.queryKey)) soon()
  })
  const hidden = () => {
    if (document.visibilityState === 'hidden') save()
  }
  document.addEventListener('visibilitychange', hidden)
  soon()
  return () => {
    unsubscribe()
    document.removeEventListener('visibilitychange', hidden)
    if (timer) clearTimeout(timer)
  }
}

/** Forget every kept copy on this device. */
export async function clearOffline(): Promise<void> {
  generation += 1
  setOfflineOwner(null)
  await store.clear().catch(() => forgetOffline())
}
