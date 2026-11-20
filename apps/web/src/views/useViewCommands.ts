import { useQueryClient } from '@tanstack/react-query'
import { useCallback, useEffect, useMemo, useRef } from 'react'

import { openShortcutSheet, usePageCommands, type PageCommand } from '../shell/commandStore'
import { describeError, showToast } from '../ui'
import { isTypingTarget } from '../work-items/shortcutKeys'
import { undoLast, useLastChange } from './saveQueue'
import { isUndoKey } from './viewKeys'

/**
 * Undo for views (T1.27): takes back the last change made in a list, board,
 * table or panel, and says what it undid or why it couldn't.
 */
export function useUndo() {
  const client = useQueryClient()
  const change = useLastChange()
  const run = useCallback(async () => {
    try {
      const undone = await undoLast(client)
      showToast(undone ? { tone: 'success', title: `Undone: ${undone.label}` } : { tone: 'info', title: 'Nothing to undo' })
    } catch (error) {
      showToast({ tone: 'danger', title: 'That wasn’t undone', description: describeError(error).message })
    }
  }, [client])
  return { change, run }
}

/** ⌘Z (Ctrl+Z elsewhere) undoes the last change, while no field, menu or dialog wants the keys. */
function useUndoKey(run: () => Promise<void>) {
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.defaultPrevented || !isUndoKey(event) || isTypingTarget(event.target)) return
      if (document.querySelector('.aw-dialog-layer, .aw-popover')) return
      event.preventDefault()
      void run()
    }
    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [run])
}

/**
 * A view's actions in the command menu (T1.27), under "This view": what
 * the view passes, then Undo for the last change and this view's keys. ⌘Z
 * undoes while the view is on screen. Pass a stable `source` per view.
 * The menu is told again only when what it lists changes (a label, a key),
 * not each time the view draws; choosing one runs the view's newest action.
 */
export function useViewCommands(source: string, commands: PageCommand[]) {
  const { change, run } = useUndo()
  useUndoKey(run)
  const all: PageCommand[] = [
    ...commands,
    ...(change ? [{ id: `${source}:undo`, label: `Undo: ${change.label}`, icon: 'undo' as const, shortcut: '⌘ Z', keywords: ['undo', 'revert', 'take back'], onSelect: () => void run() }] : []),
    { id: `${source}:keys`, label: 'Keyboard shortcuts for this view', icon: 'info' as const, shortcut: '?', onSelect: openShortcutSheet },
  ]
  const latest = useRef(all)
  useEffect(() => {
    latest.current = all
  })
  const listed = JSON.stringify(all.map(({ id, label, group, icon, shortcut, keywords, hint }) => ({ id, label, group, icon, shortcut, keywords, hint })))
  const published = useMemo<PageCommand[]>(
    () =>
      (JSON.parse(listed) as PageCommand[]).map((command) => ({
        group: 'This view',
        ...command,
        onSelect: () => latest.current.find((entry) => entry.id === command.id)?.onSelect(),
      })),
    [listed],
  )
  usePageCommands(source, published)
}

/**
 * Presses one of the view's own buttons by its name, such as the toolbar's
 * Filter or Sort, once the command menu has closed and given focus back, so
 * the menu it opens stays open.
 */
export function pressViewButton(name: string, within = '[data-view-actions]') {
  window.setTimeout(() => {
    const buttons = [...document.querySelectorAll<HTMLButtonElement>(`${within} button`)]
    const button = buttons.find((entry) => (entry.getAttribute('aria-label') ?? entry.textContent ?? '').trim() === name)
    button?.click()
  }, 0)
}

/** Puts the cursor in one of the view's fields, such as its search, once the command menu has closed. */
export function focusViewField(selector: string) {
  window.setTimeout(() => document.querySelector<HTMLElement>(selector)?.focus(), 0)
}

/** `useViewCommands` for a page that decides its actions after it has loaded: render it where they're known. */
export function ViewCommands({ commands, source }: { commands: PageCommand[]; source: string }) {
  useViewCommands(source, commands)
  return null
}
