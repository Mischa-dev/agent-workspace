// The thing store (W1.5): one copy of each thing the app has read, by kind
// and id, newest version wins. Pages read through it (`useThing`,
// `useThingList`), so a thing changed in one place shows changed everywhere
// without each page refetching.
//
// - Versions: a thing arriving with an older version than the one held is
//   ignored (a slow read can't undo a newer write).
// - Overlays: an optimistic change sits over the thing until the write
//   settles; a base update underneath keeps it, `settle` drops it (the
//   server's answer is the base by then) and `rollback` drops it after a
//   refusal, restoring exactly what the server last said.
// - Lists: a list holds ids and, optionally, a rule. A thing that arrives
//   and matches the rule is inserted in the list's sort order; one that no
//   longer matches is dropped, both without fetching. A rule that can't
//   tell (`unknown`) marks only that list stale, for its page to refetch.

import { useCallback, useSyncExternalStore } from 'react'

export type Thing = { kind: string; id: string; version?: number } & Record<string, unknown>

export type ThingRef = { kind: string; id: string }

/** Whether a thing belongs in a list: yes, no, or can't tell from here. */
export type Membership = boolean | 'unknown'

export type ListRule = {
  /** The kind of thing the list holds; others never touch it. */
  kind: string
  matches: (thing: Thing) => Membership
  /** The list's order; without one, new matches go at the end. */
  compare?: (left: Thing, right: Thing) => number
}

type Overlay = { token: number; patch: Record<string, unknown> }
type List = { ids: string[]; rule?: ListRule; stale: boolean }

const keyOf = (kind: string, id: string) => `${kind}:${id}`

/** A store of things; the app uses one (`things`), tests make their own. */
export class ThingStore {
  private base = new Map<string, Thing>()
  private overlays = new Map<string, Overlay[]>()
  private views = new Map<string, Thing>()
  private lists = new Map<string, List>()
  private listeners = new Map<string, Set<() => void>>()
  private changes = new Map<string, number>()
  private nextToken = 1

  /** The thing as shown: what the server said, with any overlays on top. */
  get(kind: string, id: string): Thing | undefined {
    return this.views.get(keyOf(kind, id))
  }

  /** Take a thing the server sent. An older version than the one held is ignored. */
  put(thing: Thing): boolean {
    const key = keyOf(thing.kind, thing.id)
    const held = this.base.get(key)
    if (held && typeof held.version === 'number' && typeof thing.version === 'number' && thing.version < held.version) {
      return false
    }
    this.base.set(key, thing)
    this.refresh(key)
    this.place(thing)
    return true
  }

  /** Take every thing in an answer (an op's value, a page of a list). */
  normalise(value: unknown): void {
    walk(value, (thing) => this.put(thing))
  }

  /** Show `patch` over the thing until the write settles. */
  overlay(ref: ThingRef, patch: Record<string, unknown>): { settle: () => void; rollback: () => void } {
    const key = keyOf(ref.kind, ref.id)
    const token = this.nextToken++
    this.overlays.set(key, [...(this.overlays.get(key) ?? []), { token, patch }])
    this.refresh(key)
    const drop = () => {
      const left = (this.overlays.get(key) ?? []).filter((overlay) => overlay.token !== token)
      if (left.length) this.overlays.set(key, left)
      else this.overlays.delete(key)
      this.refresh(key)
    }
    return { settle: drop, rollback: drop }
  }

  /** Forget a thing (it was removed, or the reader lost access). */
  remove(ref: ThingRef): void {
    const key = keyOf(ref.kind, ref.id)
    this.base.delete(key)
    this.overlays.delete(key)
    this.refresh(key)
    for (const [listKey, list] of this.lists) {
      if (list.rule?.kind === ref.kind && list.ids.includes(ref.id)) {
        list.ids = list.ids.filter((id) => id !== ref.id)
        this.emit(`list:${listKey}`)
      }
    }
  }

  /** Hold a list's ids (from a read) and its rule; it is fresh again. */
  setList(listKey: string, things: Thing[], rule?: ListRule): void {
    for (const thing of things) this.put(thing)
    this.lists.set(listKey, { ids: things.map((thing) => thing.id), rule, stale: false })
    this.emit(`list:${listKey}`)
  }

  /** The list's things, as shown. */
  list(listKey: string): Thing[] {
    const list = this.lists.get(listKey)
    if (!list) return []
    const kind = list.rule?.kind
    return list.ids.flatMap((id) => {
      const thing = kind ? this.get(kind, id) : this.findAny(id)
      return thing ? [thing] : []
    })
  }

  /** Whether the list needs reading again (a thing arrived its rule can't place). */
  isStale(listKey: string): boolean {
    return this.lists.get(listKey)?.stale ?? false
  }

  /** How many times the list has changed, for a hook's snapshot. */
  listVersion(listKey: string): number {
    return this.changes.get(`list:${listKey}`) ?? 0
  }

  subscribe(key: string, listener: () => void): () => void {
    const set = this.listeners.get(key) ?? new Set()
    set.add(listener)
    this.listeners.set(key, set)
    return () => {
      set.delete(listener)
      if (set.size === 0) this.listeners.delete(key)
    }
  }

  /** Forget everything (signing out, switching workspace). */
  clear(): void {
    const keys = [...this.views.keys()]
    this.base.clear()
    this.overlays.clear()
    this.views.clear()
    const lists = [...this.lists.keys()]
    this.lists.clear()
    for (const key of keys) this.emit(key)
    for (const key of lists) this.emit(`list:${key}`)
  }

  private findAny(id: string): Thing | undefined {
    for (const [key, thing] of this.views) if (key.endsWith(`:${id}`)) return thing
    return undefined
  }

  private refresh(key: string): void {
    const base = this.base.get(key)
    const overlays = this.overlays.get(key) ?? []
    if (!base) this.views.delete(key)
    else this.views.set(key, overlays.reduce<Thing>((thing, overlay) => ({ ...thing, ...overlay.patch }), base))
    this.emit(key)
    for (const [listKey, list] of this.lists) {
      if (base && list.rule?.kind === base.kind && list.ids.includes(base.id)) this.emit(`list:${listKey}`)
    }
  }

  /** Put a thing where each list's rule says it belongs. */
  private place(thing: Thing): void {
    for (const [listKey, list] of this.lists) {
      const rule = list.rule
      if (!rule || rule.kind !== thing.kind) continue
      const held = list.ids.includes(thing.id)
      const membership = rule.matches(thing)
      if (membership === 'unknown') {
        list.stale = true
        this.emit(`list:${listKey}`)
        continue
      }
      if (membership && !held) {
        list.ids = this.inserted(list, thing)
        this.emit(`list:${listKey}`)
      } else if (!membership && held) {
        list.ids = list.ids.filter((id) => id !== thing.id)
        this.emit(`list:${listKey}`)
      }
    }
  }

  private inserted(list: List, thing: Thing): string[] {
    const compare = list.rule?.compare
    if (!compare) return [...list.ids, thing.id]
    const kind = thing.kind
    const at = list.ids.findIndex((id) => {
      const other = this.get(kind, id)
      return other !== undefined && compare(thing, other) < 0
    })
    return at < 0 ? [...list.ids, thing.id] : [...list.ids.slice(0, at), thing.id, ...list.ids.slice(at)]
  }

  private emit(key: string): void {
    if (key.startsWith('list:')) this.changes.set(key, (this.changes.get(key) ?? 0) + 1)
    for (const listener of this.listeners.get(key) ?? []) listener()
  }
}

/** Every thing in a value: objects with a string kind and id. */
function walk(value: unknown, found: (thing: Thing) => void, depth = 0): void {
  if (depth > 6 || value === null || typeof value !== 'object') return
  if (Array.isArray(value)) {
    for (const item of value) walk(item, found, depth + 1)
    return
  }
  const record = value as Record<string, unknown>
  if (typeof record.kind === 'string' && typeof record.id === 'string' && typeof record.version === 'number') {
    found(record as Thing)
  }
  for (const nested of Object.values(record)) walk(nested, found, depth + 1)
}

/** The app's store. */
export const things = new ThingStore()

/** One thing, as shown, kept current. */
export function useThing(ref: ThingRef | null, store: ThingStore = things): Thing | undefined {
  const key = ref ? keyOf(ref.kind, ref.id) : ''
  const subscribe = useCallback((listener: () => void) => (key ? store.subscribe(key, listener) : () => {}), [key, store])
  return useSyncExternalStore(subscribe, () => (ref ? store.get(ref.kind, ref.id) : undefined))
}

/** A list's things, as shown, and whether it needs reading again. */
export function useThingList(listKey: string, store: ThingStore = things): { things: Thing[]; stale: boolean } {
  const subscribe = useCallback((listener: () => void) => store.subscribe(`list:${listKey}`, listener), [listKey, store])
  // The snapshot is the list's change count, stable between changes.
  useSyncExternalStore(subscribe, () => store.listVersion(listKey))
  return { things: store.list(listKey), stale: store.isStale(listKey) }
}
