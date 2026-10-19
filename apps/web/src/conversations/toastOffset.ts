import { useEffect, type RefObject } from 'react'

/**
 * While a conversation's composer is on screen, toasts sit above it rather
 * than over Send: the page sets `--toast-offset` on the root to the distance
 * from the composer's top to the bottom of the window, and clears it when the
 * composer goes. With a thread open beside the channel, the taller one counts.
 */
const composers = new Set<HTMLElement>()
let frame = 0

function measure() {
  frame = 0
  let offset = 0
  for (const element of composers) {
    if (!element.isConnected) continue
    const rect = element.getBoundingClientRect()
    if (rect.height === 0) continue
    offset = Math.max(offset, window.innerHeight - rect.top)
  }
  const root = document.documentElement.style
  if (offset > 0) root.setProperty('--toast-offset', `${Math.round(offset)}px`)
  else root.removeProperty('--toast-offset')
}

function schedule() {
  if (frame) return
  frame = window.requestAnimationFrame(measure)
}

export function useToastOffset(ref: RefObject<HTMLElement | null>, enabled: boolean, shape: unknown) {
  useEffect(() => {
    const element = ref.current
    if (!enabled || !element) return undefined
    composers.add(element)
    const observer = typeof ResizeObserver === 'function' ? new ResizeObserver(schedule) : null
    observer?.observe(element)
    window.addEventListener('resize', schedule)
    schedule()
    return () => {
      composers.delete(element)
      observer?.disconnect()
      window.removeEventListener('resize', schedule)
      if (frame) window.cancelAnimationFrame(frame)
      measure()
    }
    // `shape` is what swaps the composer for another element, such as archiving.
  }, [enabled, ref, shape])
}
