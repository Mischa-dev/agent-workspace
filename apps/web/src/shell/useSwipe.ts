import { useEffect, useRef, type RefObject } from 'react'

const MIN_DISTANCE = 60
const MAX_DURATION_MS = 600

/** True when the touch began somewhere a sideways drag already means something else. */
function claimsHorizontalDrag(target: Element) {
  if (target.closest('input, textarea, select, [contenteditable="true"], [role="slider"]')) return true
  for (let element: Element | null = target; element; element = element.parentElement) {
    if (element.scrollWidth > element.clientWidth) {
      const overflow = getComputedStyle(element).overflowX
      if (overflow === 'auto' || overflow === 'scroll') return true
    }
  }
  return false
}

/**
 * Quick horizontal flicks on touch screens. A slow drag, a mostly vertical
 * scroll, a text selection, or a drag inside something that scrolls sideways
 * (code, tables, the board) is left alone.
 */
export function useSwipe(
  ref: RefObject<HTMLElement | null>,
  handlers: { onSwipeLeft?: (target: Element) => void; onSwipeRight?: (target: Element) => void },
) {
  const latest = useRef(handlers)
  useEffect(() => {
    latest.current = handlers
  })

  useEffect(() => {
    const element = ref.current
    if (!element) return undefined
    let start: { x: number; y: number; time: number; target: Element } | null = null

    const onStart = (event: TouchEvent) => {
      const touch = event.touches[0]
      const target = event.target instanceof Element ? event.target : null
      start =
        event.touches.length === 1 && touch && target && !claimsHorizontalDrag(target)
          ? { x: touch.clientX, y: touch.clientY, time: event.timeStamp, target }
          : null
    }
    const onEnd = (event: TouchEvent) => {
      const touch = event.changedTouches[0]
      const began = start
      start = null
      if (!began || !touch || event.timeStamp - began.time > MAX_DURATION_MS) return
      if (window.getSelection()?.toString()) return
      const dx = touch.clientX - began.x
      const dy = touch.clientY - began.y
      if (Math.abs(dx) < MIN_DISTANCE || Math.abs(dy) > Math.abs(dx) / 2) return
      if (dx > 0) latest.current.onSwipeRight?.(began.target)
      else latest.current.onSwipeLeft?.(began.target)
    }
    const onCancel = () => {
      start = null
    }

    element.addEventListener('touchstart', onStart, { passive: true })
    element.addEventListener('touchend', onEnd, { passive: true })
    element.addEventListener('touchcancel', onCancel, { passive: true })
    return () => {
      element.removeEventListener('touchstart', onStart)
      element.removeEventListener('touchend', onEnd)
      element.removeEventListener('touchcancel', onCancel)
    }
  }, [ref])
}
