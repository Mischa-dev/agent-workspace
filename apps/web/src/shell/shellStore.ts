import { createContext, useContext, useId, useLayoutEffect, type ReactNode } from 'react'

import type { ShellPresence } from './AppShell'

// W1.11: the persistent shell. `ShellLayout` draws the rail, the sidebar and
// the panel once around every shell page; each page says what it needs
// beside it (its space, its panel) through these hooks, and the shell keeps
// its DOM from page to page.

/** Each page part's say, by who said it: the shell merges them in order. */
export type ShellRegistrar = (id: string, presence: ShellPresence | null) => void

export const ShellPresenceContext = createContext<ShellRegistrar | null>(null)

/** Merges each registrant's presence, latest registrant last. */
export function mergePresence(entries: Iterable<ShellPresence>): ShellPresence {
  return Object.assign({}, ...entries) as ShellPresence
}

/** Whether two presences draw the same shell. */
export function samePresence(left: ShellPresence, right: ShellPresence) {
  const keys = new Set([...Object.keys(left), ...Object.keys(right)] as (keyof ShellPresence)[])
  for (const key of keys) if (left[key] !== right[key]) return false
  return true
}

/**
 * Tells the persistent shell what this page needs beside it, for as long as
 * it's shown. Returns whether there is a persistent shell to tell; without
 * one (a page rendered on its own) the caller draws its own frame.
 */
export function useShellPresence(presence: ShellPresence): boolean {
  const register = useContext(ShellPresenceContext)
  const id = useId()
  // Before paint, so the sidebar never shows the last page's space.
  useLayoutEffect(() => {
    register?.(id, presence)
  })
  useLayoutEffect(() => {
    if (!register) return undefined
    return () => register(id, null)
  }, [id, register])
  return register !== null
}

/** The details panel beside this page (thread details, group members). `key` changes when the person asks to see something new in it. */
export function useShellPanel(panel: ReactNode, key?: string, onDismiss?: () => void) {
  return useShellPresence({ context: panel, contextKey: key, onContextDismiss: onDismiss })
}

/** The space this page belongs to: a project, Messages or Home. */
export function useShellSpace(space: Pick<ShellPresence, 'project' | 'direct' | 'home' | 'selectedChannelId' | 'thread'>) {
  return useShellPresence(space)
}
