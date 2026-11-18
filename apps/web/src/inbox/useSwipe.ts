import { useRef, useState, type PointerEvent, type SyntheticEvent } from 'react'

/** How far a row moves before letting go acts. */
export const SWIPE_THRESHOLD = 80
const MAX_SHIFT = 140
/** A move this far, mostly sideways, makes the gesture a swipe, not a scroll. */
const SLOP = 10

/** Which way a sideways move of `dx` by `dy` goes, once it's clear. */
export function swipeAxis(dx: number, dy: number): 'x' | 'y' | null {
  if (Math.abs(dx) > SLOP && Math.abs(dx) > Math.abs(dy) * 1.5) return 'x'
  if (Math.abs(dy) > SLOP) return 'y'
  return null
}

/** What letting go at `dx` does. */
export function swipeOutcome(dx: number): 'left' | 'right' | null {
  if (dx <= -SWIPE_THRESHOLD) return 'left'
  if (dx >= SWIPE_THRESHOLD) return 'right'
  return null
}

/**
 * Swipe a row sideways with a finger (C2.31): left and right each do
 * something once the row has moved far enough. Mouse and pen are left
 * alone, and a swipe never also counts as a tap on the row.
 */
export function useSwipe({ onLeft, onRight }: { onLeft?: () => void; onRight?: () => void }) {
  const [shift, setShift] = useState(0)
  const gesture = useRef<{ x: number; y: number; id: number; axis: 'x' | 'y' | null } | null>(null)
  const swiped = useRef(false)
  const reset = () => {
    gesture.current = null
    setShift(0)
  }
  return {
    shift,
    outcome: swipeOutcome(shift),
    handlers: {
      onPointerDown: (event: PointerEvent<HTMLElement>) => {
        if (event.pointerType !== 'touch') return
        swiped.current = false
        gesture.current = { x: event.clientX, y: event.clientY, id: event.pointerId, axis: null }
      },
      onPointerMove: (event: PointerEvent<HTMLElement>) => {
        const current = gesture.current
        if (!current || current.id !== event.pointerId) return
        const dx = event.clientX - current.x
        current.axis ??= swipeAxis(dx, event.clientY - current.y)
        if (current.axis !== 'x') return
        const limited = Math.max(onLeft ? -MAX_SHIFT : 0, Math.min(onRight ? MAX_SHIFT : 0, dx))
        setShift(limited)
      },
      onPointerUp: (event: PointerEvent<HTMLElement>) => {
        const current = gesture.current
        if (!current || current.id !== event.pointerId) return
        if (current.axis === 'x') {
          swiped.current = true
          const outcome = swipeOutcome(shift)
          if (outcome === 'left') onLeft?.()
          if (outcome === 'right') onRight?.()
        }
        reset()
      },
      onPointerCancel: reset,
      onClickCapture: (event: SyntheticEvent) => {
        if (!swiped.current) return
        swiped.current = false
        event.preventDefault()
        event.stopPropagation()
      },
    },
  }
}
