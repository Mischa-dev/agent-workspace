import { useEffect, useRef } from 'react'

export const LIST_SHORTCUTS: Array<{ keys: string[]; label: string }> = [
  { keys: ['J', 'K'], label: 'Move down and up' },
  { keys: ['X'], label: 'Select or unselect' },
  { keys: ['Shift', 'click'], label: 'Select a range' },
  { keys: ['S'], label: 'Change status' },
  { keys: ['A'], label: 'Assign' },
  { keys: ['L'], label: 'Change labels' },
  { keys: ['P'], label: 'Change priority' },
  { keys: ['M'], label: 'Set milestone' },
  { keys: ['Enter'], label: 'Open in the side panel' },
  { keys: ['Esc'], label: 'Close the panel, then clear the selection' },
  { keys: ['?'], label: 'Show these shortcuts' },
]

export const BOARD_SHORTCUTS: Array<{ keys: string[]; label: string }> = [
  { keys: ['Arrows'], label: 'Move between cards' },
  { keys: ['Space'], label: 'Pick up a card, then arrows to move it and Space to drop' },
  { keys: ['S', 'A', 'L', 'P', 'M'], label: 'Status, assignee, labels, priority, milestone' },
  { keys: ['Enter'], label: 'Open in the side panel' },
  { keys: ['Esc'], label: 'Put a picked-up card back, or close the panel' },
]

/** True when a key press belongs to something else: a field, a menu, a dialog or a picker. */
export function isTypingTarget(target: EventTarget | null) {
  if (!(target instanceof HTMLElement)) return false
  if (target.isContentEditable) return true
  if (target.closest('input, textarea, select, [role="dialog"], [role="menu"], .aw-popover, #context-region')) return true
  return false
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

