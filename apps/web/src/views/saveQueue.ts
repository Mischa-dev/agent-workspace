import type { QueryClient } from '@tanstack/react-query'
import { useSyncExternalStore } from 'react'

/**
 * The one save queue for changes made in lists, boards, tables and panels
 * (T1.27). Tasks and records save through it, so a change made in a list and
 * one made in the side panel never cross: one thing's saves go one at a
 * time, in the order they were made, whichever view made them. A save that
 * fails doesn't hold up the ones behind it.
 *
 * It also keeps the last change made, so Undo (⌘Z, or the command menu)
 * can take it back. Undoing is a change like any other: it's saved as a
 * new version, and refused when someone changed the same thing since.
 */

const queues = new Map<string, Promise<unknown>>()

/** Runs `save` once every earlier save of `thing` (such as "task:<id>") has settled. */
export function inOrder<T>(thing: string, save: () => Promise<T>): Promise<T> {
  const before = queues.get(thing) ?? Promise.resolve()
  const next = before.then(save, save)
  queues.set(thing, next)
  const forget = () => {
    if (queues.get(thing) === next) queues.delete(thing)
  }
  next.then(forget, forget)
  return next
}

/** Whether `thing` has saves waiting or on their way. */
export function saving(thing: string) {
  return queues.has(thing)
}

/** A change that can be taken back. */
export type Undoable = {
  /** What it did, for the menu and the note after undoing: "Changed the status of AWD-12". */
  label: string
  /** Takes it back, then refreshes what it changed in `client`; throws, with a message worth showing, when it can't. */
  undo: (client: QueryClient) => Promise<void>
}

let last: Undoable | null = null
const listeners = new Set<() => void>()

function announce() {
  for (const listener of listeners) listener()
}

/** Keeps `change` as the one Undo takes back. */
export function remember(change: Undoable) {
  last = change
  announce()
}

/** Forgets the last change, such as when it couldn't be saved. */
export function forget(change?: Undoable) {
  if (change && last !== change) return
  last = null
  announce()
}

export function lastChange() {
  return last
}

/** The change Undo would take back, for a page that offers it. */
export function useLastChange() {
  return useSyncExternalStore(
    (listener) => {
      listeners.add(listener)
      return () => listeners.delete(listener)
    },
    () => last,
    () => last,
  )
}

/**
 * Takes back the last change: what it was, or null when there was nothing
 * to undo. Throws when it can't be undone; either way it's forgotten, so a
 * second Undo doesn't try again.
 */
export async function undoLast(client: QueryClient): Promise<Undoable | null> {
  const change = last
  if (!change) return null
  last = null
  announce()
  await change.undo(client)
  return change
}

/** For tests. */
export function resetSaveQueue() {
  queues.clear()
  last = null
  announce()
}
