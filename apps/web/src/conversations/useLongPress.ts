import { useEffect, useRef, type PointerEvent } from 'react'

/** How long a finger rests before it counts as a long press. */
export const LONG_PRESS_MS = 450
const SLOP = 10

/**
 * A finger held still on something (C2.31): `onPress` runs once it has
 * rested long enough. Moving, lifting or a mouse or pen does nothing.
 */
export function useLongPress(onPress: (() => void) | undefined) {
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const start = useRef<{ x: number; y: number } | null>(null)
  const cancel = () => {
    if (timer.current) clearTimeout(timer.current)
    timer.current = null
    start.current = null
  }
  useEffect(() => cancel, [])
  if (!onPress) return {}
  return {
    onPointerDown: (event: PointerEvent<HTMLElement>) => {
      if (event.pointerType !== 'touch') return
      cancel()
      start.current = { x: event.clientX, y: event.clientY }
      timer.current = setTimeout(() => {
        timer.current = null
        onPress()
      }, LONG_PRESS_MS)
    },
    onPointerMove: (event: PointerEvent<HTMLElement>) => {
      const at = start.current
      if (at && Math.hypot(event.clientX - at.x, event.clientY - at.y) > SLOP) cancel()
    },
    onPointerUp: cancel,
    onPointerCancel: cancel,
  }
}
