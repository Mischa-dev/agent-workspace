import { useEffect, useMemo, useRef } from 'react'

import { isMobileWidth } from '../shell/mobileNavigation'

export const LIST_SHORTCUTS: Array<{ keys: string[]; label: string }> = [
  { keys: ['↓', '↑'], label: 'Next and previous task, opening each in the side panel' },
  { keys: ['J', 'K'], label: 'The same, wherever you are on the page' },
  { keys: ['Home', 'End'], label: 'First and last task' },
  { keys: ['X'], label: 'Select or unselect' },
  { keys: ['Shift', 'click'], label: 'Select a range' },
  { keys: ['S'], label: 'Change status' },
  { keys: ['A'], label: 'Assign' },
  { keys: ['L'], label: 'Change labels' },
  { keys: ['P'], label: 'Change priority' },
  { keys: ['M'], label: 'Set milestone' },
  { keys: ['Enter'], label: 'Open in the side panel' },
  { keys: ['Esc'], label: 'Close the panel, then clear the selection' },
  { keys: ['?'], label: 'All keyboard shortcuts' },
]

export const BOARD_SHORTCUTS: Array<{ keys: string[]; label: string }> = [
  { keys: ['←', '→'], label: 'Next and previous column, opening each card in the side panel' },
  { keys: ['↓', '↑'], label: 'Next and previous card in a column' },
  { keys: ['J', 'K'], label: 'Next and previous card, down each column' },
  { keys: ['Home', 'End'], label: 'First and last card in a column' },
  { keys: ['X'], label: 'Select or unselect' },
  { keys: ['Shift', 'click'], label: 'Select a range' },
  { keys: ['Space'], label: 'Pick up a card, then arrows to move it and Space to drop' },
  { keys: ['S', 'A', 'L', 'P', 'M'], label: 'Status, assignee, labels, priority, milestone' },
  { keys: ['Enter'], label: 'Open in the side panel' },
  { keys: ['Esc'], label: 'Put a picked-up card back, close the panel, then clear the selection' },
]

/** True when a key press belongs to something else: a field, a menu, a dialog or a picker. */
export function isTypingTarget(target: EventTarget | null) {
  if (!(target instanceof HTMLElement)) return false
  if (target.isContentEditable) return true
  if (target.closest('input, textarea, select, [role="dialog"], [role="menu"], .aw-popover, #context-region')) return true
  return false
}

/**
 * True when nothing in particular has focus (the page itself, or its main
 * region after a click on empty space), so the arrow keys can start moving
 * through the list instead of scrolling.
 */
export function pageHasFocus() {
  const active = document.activeElement
  return !active || active === document.body || active.id === 'main-workspace'
}

/** How long the keyboard selection rests on a task before the side panel follows it. */
const FOLLOW_DELAY_MS = 120

/**
 * Opens what the keyboard selection lands on in the side panel, once it stops
 * for a moment, so holding an arrow key doesn't load every task on the way.
 * On a phone the panel would cover the list, so Enter opens it there.
 */
export function useOpenAsYouGo<T = string>(open: (target: T) => void) {
  const openRef = useRef(open)
  const timer = useRef<number | undefined>(undefined)
  useEffect(() => {
    openRef.current = open
  }, [open])
  useEffect(() => () => window.clearTimeout(timer.current), [])
  return useMemo(
    () => ({
      follow(target: T) {
        window.clearTimeout(timer.current)
        if (isMobileWidth()) return
        timer.current = window.setTimeout(() => openRef.current(target), FOLLOW_DELAY_MS)
      },
      /** Forget a pending open: Escape, a click or Enter came first. */
      cancel() {
        window.clearTimeout(timer.current)
      },
    }),
    [],
  )
}

/**
 * Single-key shortcuts for a task view, active while nothing else wants the
 * keys. The handler returns true when it used the key.
 */
export function useTaskShortcuts(enabled: boolean, handler: (event: KeyboardEvent) => boolean) {
  const handlerRef = useRef(handler)
  useEffect(() => {
    handlerRef.current = handler
  }, [handler])
  useEffect(() => {
    if (!enabled) return undefined
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.defaultPrevented || event.altKey) return
      if (isTypingTarget(event.target)) return
      if (document.querySelector('.aw-dialog-layer, .aw-popover')) return
      if (handlerRef.current(event)) event.preventDefault()
    }
    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [enabled])
}

