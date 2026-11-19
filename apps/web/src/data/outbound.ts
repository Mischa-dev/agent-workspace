// The outbound queue (W1.30): a write made while the connection is down is
// kept on this device, in IndexedDB, and sent once the connection is back,
// with the request id it was first made with, so a write that did reach the
// server is answered once. Writes to one thing go in the order they were
// made; one that's refused stays with its reason until the person retries
// or drops it. Only the signed-in account's writes are kept, and signing
// out forgets them.

import { useSyncExternalStore } from 'react'

import { callOp, newRequestId, type OpInput, type OpOutput } from '../api/ops'
import type { OpName } from '../api/generated/operations'
import { describeError } from '../ui/errorText'
import { isConnectionError, localDraftOwner } from '../app/localDrafts'

/**
 * The operations that may wait for the connection. A stub until K2's
 * registry marks operations `queueable`; then the generated flag replaces it.
 */
export const QUEUEABLE: ReadonlySet<OpName> = new Set<OpName>([
  'message_send',
  'task_update',
  'task_comment_create',
  'comment_create',
])

export type OutboundEntry = {
  /** The request id it's sent with, every time. */
  id: string
  owner: string
  op: OpName
  input: unknown
  /** `kind:id` of the thing it changes; writes to one thing keep their order. */
  thing: string | null
  /** Where it shows, for "Open". */
  href: string | null
  /** What it was, in words ("Message in #general"). */
  label: string
  createdAt: string
  /** The order it was made in, among everything waiting. */
  order: number
  /** The server said no: why, in a sentence. */
  refused?: string
}

export type OutboundMeta = { thing?: string | null; href?: string | null; label: string }

// --- Where entries are kept --------------------------------------------------

const DATABASE = 'agent-workspace-outbound'
const STORE = 'entries'

type Store = {
  all(): Promise<OutboundEntry[]>
  put(entry: OutboundEntry): Promise<void>
  remove(id: string): Promise<void>
  clear(): Promise<void>
}

function memoryStore(): Store {
  const kept = new Map<string, OutboundEntry>()
  return {
    all: async () => [...kept.values()],
    put: async (entry) => {
      kept.set(entry.id, entry)
    },
    remove: async (id) => {
      kept.delete(id)
    },
    clear: async () => kept.clear(),
  }
}

function request<T>(open: () => IDBRequest<T>): Promise<T> {
  return new Promise((resolve, reject) => {
    const pending = open()
    pending.onsuccess = () => resolve(pending.result)
    pending.onerror = () => reject(pending.error)
  })
}

function indexedStore(factory: IDBFactory): Store {
  let database: Promise<IDBDatabase> | null = null
  const open = () => {
    database ??= new Promise((resolve, reject) => {
      const opening = factory.open(DATABASE, 1)
      opening.onupgradeneeded = () => {
        if (!opening.result.objectStoreNames.contains(STORE)) opening.result.createObjectStore(STORE, { keyPath: 'id' })
      }
      opening.onsuccess = () => resolve(opening.result)
      opening.onerror = () => {
        database = null
        reject(opening.error)
      }
    })
    return database
  }
  const run = async <T>(mode: IDBTransactionMode, work: (store: IDBObjectStore) => IDBRequest<T>) => {
    const db = await open()
    return request(() => work(db.transaction(STORE, mode).objectStore(STORE)))
  }
  return {
    all: () => run('readonly', (store) => store.getAll() as IDBRequest<OutboundEntry[]>),
    put: (entry) => run('readwrite', (store) => store.put(entry)).then(() => undefined),
    remove: (id) => run('readwrite', (store) => store.delete(id)).then(() => undefined),
    clear: () => run('readwrite', (store) => store.clear()).then(() => undefined),
  }
}

let store: Store = typeof indexedDB === 'undefined' ? memoryStore() : indexedStore(indexedDB)

/** For tests: start from an empty store kept in memory. */
export function resetOutboundForTests() {
  store = memoryStore()
  entries = []
  loaded = null
  loadedFor = null
  emit()
}

// --- What's waiting, as the app reads it ------------------------------------

let entries: OutboundEntry[] = []
let loaded: Promise<void> | null = null
let loadedFor: string | null = null
const listeners = new Set<() => void>()

function emit() {
  for (const listener of listeners) listener()
}

// Request ids made in the same millisecond don't sort by when; this does.
let made = 0
const nextOrder = () => Date.now() * 1_000 + (made++ % 1_000)

function setEntries(next: OutboundEntry[]) {
  entries = [...next].sort((left, right) => left.order - right.order)
  emit()
}

/** Reads what's kept for whoever is signed in, once; anyone else's goes. */
function load() {
  const owner = localDraftOwner()
  if (loadedFor !== owner) {
    // Someone else signed in: start from what's kept for them.
    loaded = null
    loadedFor = owner
    entries = entries.filter((entry) => entry.owner === owner)
  }
  loaded ??= store
    .all()
    .then(async (all) => {
      const owner = localDraftOwner()
      const others = all.filter((entry) => entry.owner !== owner)
      await Promise.all(others.map((entry) => store.remove(entry.id)))
      setEntries([...entries.filter((entry) => entry.owner === owner), ...all.filter((entry) => entry.owner === owner && !entries.some((known) => known.id === entry.id))])
    })
    .catch(() => undefined)
  return loaded
}

function subscribe(listener: () => void) {
  listeners.add(listener)
  void load()
  return () => {
    listeners.delete(listener)
  }
}

const snapshot = () => entries

/** What's waiting to be sent (or was refused), oldest first; for one thing when given. */
export function useOutbound(thing?: string): OutboundEntry[] {
  const all = useSyncExternalStore(subscribe, snapshot, snapshot)
  return thing ? all.filter((entry) => entry.thing === thing) : all
}

export function outboundEntries() {
  return entries
}

// --- Writing ----------------------------------------------------------------

async function keep(entry: OutboundEntry) {
  setEntries([...entries.filter((known) => known.id !== entry.id), entry])
  await store.put(entry).catch(() => undefined)
}

async function forget(id: string) {
  setEntries(entries.filter((entry) => entry.id !== id))
  await store.remove(id).catch(() => undefined)
}

/** Whether an earlier write to the same thing is still waiting. */
function waitingBefore(entry: OutboundEntry) {
  return Boolean(entry.thing) && entries.some((other) => other.thing === entry.thing && other.order < entry.order)
}

/**
 * Make a write that may wait for the connection. It's kept first, then sent;
 * sent, it's forgotten and its answer returned. Offline (or behind an earlier
 * write to the same thing still waiting) it stays kept and this returns
 * `null`: the write is queued. A refusal while sending now throws, as a
 * plain `callOp` would, and nothing is kept.
 */
export async function queueOp<N extends OpName>(
  op: N,
  input: OpInput<N>,
  meta: OutboundMeta,
  options: { requestId?: string } = {},
): Promise<OpOutput<N> | null> {
  const owner = localDraftOwner()
  if (!owner || !QUEUEABLE.has(op)) return callOp(op, input, { requestId: options.requestId })
  await load()
  const entry: OutboundEntry = {
    id: options.requestId ?? newRequestId(),
    owner,
    op,
    input,
    thing: meta.thing ?? null,
    href: meta.href ?? null,
    label: meta.label,
    createdAt: new Date().toISOString(),
    order: nextOrder(),
  }
  await keep(entry)
  if (waitingBefore(entry)) return null
  try {
    const answer = await callOp(op, input, { requestId: entry.id, retries: 0 })
    await forget(entry.id)
    return answer
  } catch (error) {
    if (isConnectionError(error)) return null
    await forget(entry.id)
    throw error
  }
}

export type FlushResult = { sent: OutboundEntry[]; refused: OutboundEntry[] }

let flushing: Promise<FlushResult> | null = null

/**
 * Send what's waiting, oldest first, each with its own request id. A thing
 * whose earlier write is refused or still can't go holds its later ones
 * back; a lost connection stops the run. Runs one at a time.
 */
export function flushOutbound(): Promise<FlushResult> {
  flushing ??= (async () => {
    const result: FlushResult = { sent: [], refused: [] }
    await load()
    const owner = localDraftOwner()
    const held = new Set<string>()
    for (const entry of [...entries]) {
      if (entry.owner !== owner) continue
      if (entry.refused || (entry.thing && held.has(entry.thing))) {
        if (entry.thing) held.add(entry.thing)
        continue
      }
      try {
        await callOp(entry.op, entry.input as never, { requestId: entry.id, retries: 0 })
      } catch (error) {
        if (isConnectionError(error) || localDraftOwner() !== owner) return result
        const refused = { ...entry, refused: describeError(error).message }
        await keep(refused)
        result.refused.push(refused)
        if (entry.thing) held.add(entry.thing)
        continue
      }
      await forget(entry.id)
      result.sent.push(entry)
    }
    return result
  })().finally(() => {
    flushing = null
  })
  return flushing
}

/** Try a refused write again, as it was. */
export async function retryOutbound(id: string) {
  const entry = entries.find((candidate) => candidate.id === id)
  if (!entry) return
  await keep({ ...entry, refused: undefined })
  return flushOutbound()
}

/** Drop a waiting or refused write. */
export function discardOutbound(id: string) {
  return forget(id)
}

/** Forget everything waiting on this device: signing out. */
export async function clearOutbound() {
  setEntries([])
  loaded = null
  loadedFor = null
  await store.clear().catch(() => undefined)
}
