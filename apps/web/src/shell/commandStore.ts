import { useEffect, useSyncExternalStore } from 'react'

import type { CommandItem } from '../ui'

/**
 * What the command menu shows beyond the app-wide commands: actions a page
 * offers for what's on screen (archive this channel, stop this run), and
 * whether the menu or the shortcut sheet is open. Kept outside React so any
 * page or shortcut can open the menu.
 */

export type PageCommand = Omit<CommandItem, 'group'> & { group?: string }

type State = {
  open: 'commands' | 'shortcuts' | null
  /** The text the menu opens with, such as "Create task". */
  initialQuery: string
  page: Record<string, PageCommand[]>
}

let state: State = { open: null, initialQuery: '', page: {} }
const listeners = new Set<() => void>()

function set(next: Partial<State>) {
  state = { ...state, ...next }
  for (const listener of listeners) listener()
}

function subscribe(listener: () => void) {
  listeners.add(listener)
  return () => listeners.delete(listener)
}

export function useCommandState() {
  return useSyncExternalStore(subscribe, () => state, () => state)
}

export function openCommandMenu(initialQuery = '') {
  set({ open: 'commands', initialQuery })
}

export function openShortcutSheet() {
  set({ open: 'shortcuts', initialQuery: '' })
}

export function closeCommandMenu() {
  if (state.open) set({ open: null, initialQuery: '' })
}

/**
 * Offer commands in the menu while the calling component is mounted, under
 * "This page" unless they name a group. Pass a stable `source` per component.
 */
export function usePageCommands(source: string, commands: PageCommand[]) {
  useEffect(() => {
    set({ page: { ...state.page, [source]: commands } })
    return () => {
      const page = { ...state.page }
      delete page[source]
      set({ page })
    }
  }, [commands, source])
}

export function pageCommands(page: State['page']): CommandItem[] {
  return Object.values(page)
    .flat()
    .map((command) => ({ ...command, group: command.group ?? 'This page' }))
}

/** For tests. */
export function resetCommandState() {
  state = { open: null, initialQuery: '', page: {} }
  for (const listener of listeners) listener()
}
