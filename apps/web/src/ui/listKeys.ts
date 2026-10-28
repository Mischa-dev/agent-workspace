import { useEffect, useRef, type RefObject } from 'react'

/** A field, menu, dialog, picker or side panel has the key, not the list. */
function belongsElsewhere(target: EventTarget | null) {
  if (!(target instanceof HTMLElement)) return false
  if (target.isContentEditable) return true
  return Boolean(target.closest('input, textarea, select, [role="dialog"], [role="menu"], [role="listbox"], .aw-popover, #context-region'))
}

/**
 * Arrow keys through a list of links: Down and Up (or J and K) move focus to
 * the next and previous one, Home and End to the ends. J and K work from
 * anywhere but a field; the arrows, Home and End only once focus is in the
 * list or nowhere in particular, so everywhere else they keep scrolling.
 * `onMove` hears where focus landed; `onEscape` returns true when Escape
 * closed something.
 */
export function useListKeys(
  listRef: RefObject<HTMLElement | null>,
  selector: string,
  { onEscape, onMove }: { onEscape?: () => boolean; onMove?: (element: HTMLElement) => void } = {},
) {
  const handlers = useRef({ onEscape, onMove })
  useEffect(() => {
    handlers.current = { onEscape, onMove }
  }, [onEscape, onMove])
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.defaultPrevented || event.altKey || event.metaKey || event.ctrlKey || event.shiftKey) return
      if (belongsElsewhere(event.target) || document.querySelector('.aw-dialog-layer, .aw-popover')) return
      const list = listRef.current
      if (!list) return
      if (event.key === 'Escape') {
        if (handlers.current.onEscape?.()) event.preventDefault()
        return
      }
      const active = document.activeElement
      const inList = active instanceof HTMLElement && list.contains(active)
      const free = inList || !active || active === document.body || active.id === 'main-workspace'
      const key = event.key
      const step = key === 'j' || (key === 'ArrowDown' && free) ? 1 : key === 'k' || (key === 'ArrowUp' && free) ? -1 : 0
      const end = (key === 'Home' || key === 'End') && free ? key : null
      if (!step && !end) return
      const items = [...list.querySelectorAll<HTMLElement>(selector)]
      if (!items.length) return
      event.preventDefault()
      const index = inList ? items.findIndex((item) => item === active || item.contains(active)) : -1
      const next = end === 'Home' ? 0 : end === 'End' ? items.length - 1 : index < 0 ? 0 : Math.max(0, Math.min(items.length - 1, index + step))
      items[next].focus()
      handlers.current.onMove?.(items[next])
    }
    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [listRef, selector])
}
