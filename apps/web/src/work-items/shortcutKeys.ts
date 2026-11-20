import { useEffect, useMemo, useRef } from 'react'

import { isMobileWidth } from '../shell/mobileNavigation'
import { viewKeys } from '../views/viewKeys'

/** The task list's and board's keys, from the one list of view keys (T1.27). */
export const LIST_SHORTCUTS: Array<{ keys: string[]; label: string }> = viewKeys('list')
export const BOARD_SHORTCUTS: Array<{ keys: string[]; label: string }> = viewKeys('board')

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

