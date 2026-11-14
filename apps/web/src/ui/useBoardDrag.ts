import { useCallback, useEffect, useRef, useState, type PointerEvent as ReactPointerEvent, type RefObject } from 'react'

export type DropTarget = { columnKey: string; laneKey: string; index: number }

export type DragState = { cardId: string; target: DropTarget | null; width: number }

type Pending = {
  cardId: string
  pointerId: number
  pointerType: string
  startX: number
  startY: number
  offsetX: number
  offsetY: number
  width: number
  timer: number | undefined
}

const MOUSE_SLOP = 4
const TOUCH_SLOP = 8
const LONG_PRESS = 280
const EDGE = 48
const SPEED = 14

/**
 * The drop slot under a point: the cell, and how many of its other cards are
 * above the point. Cards carry `data-drag-id` (column and task, since a task
 * can show in two label columns).
 */
export function targetAt(x: number, y: number, draggedId: string): DropTarget | null {
  const element = document.elementFromPoint(x, y)
  const cell = element?.closest<HTMLElement>('[data-drop-cell]')
  if (!cell) return null
  const cards = [...cell.querySelectorAll<HTMLElement>('[data-drag-id]')].filter((card) => card.dataset.dragId !== draggedId)
  let index = cards.length
  for (let position = 0; position < cards.length; position += 1) {
    const rect = cards[position].getBoundingClientRect()
    if (y < rect.top + rect.height / 2) {
      index = position
      break
    }
  }
  return { columnKey: cell.dataset.column ?? '', laneKey: cell.dataset.lane ?? 'all', index }
}

/**
 * Pointer dragging for board cards. A mouse drags after a few pixels; a finger
 * holds still for a moment first, so a swipe still scrolls the board. While a
 * card is held, the board (or the page) scrolls near its edges.
 */
export function useBoardDrag({
  enabled,
  ghostRef,
  onDrop,
  scrollerRef,
}: {
  enabled: boolean
  ghostRef: RefObject<HTMLElement | null>
  onDrop: (cardId: string, target: DropTarget) => void
  scrollerRef: RefObject<HTMLElement | null>
}) {
  const [drag, setDrag] = useState<DragState | null>(null)
  const pending = useRef<Pending | null>(null)
  const dragging = useRef<DragState | null>(null)
  const pointer = useRef({ x: 0, y: 0 })
  const suppressClick = useRef(0)
  const onDropRef = useRef(onDrop)
  useEffect(() => {
    onDropRef.current = onDrop
  }, [onDrop])

  const moveGhost = useCallback(() => {
    const ghost = ghostRef.current
    const current = pending.current
    if (!ghost || !current) return
    ghost.style.transform = `translate(${pointer.current.x - current.offsetX}px, ${pointer.current.y - current.offsetY}px)`
  }, [ghostRef])

  const finish = useCallback(() => {
    const current = pending.current
    if (current?.timer) window.clearTimeout(current.timer)
    pending.current = null
    dragging.current = null
    setDrag(null)
  }, [])

  const start = useCallback(() => {
    const current = pending.current
    if (!current) return
    const state: DragState = { cardId: current.cardId, target: targetAt(pointer.current.x, pointer.current.y, current.cardId), width: current.width }
    dragging.current = state
    setDrag(state)
    if (current.pointerType === 'touch') navigator.vibrate?.(10)
    window.requestAnimationFrame(moveGhost)
  }, [moveGhost])

  useEffect(() => {
    if (!enabled) return undefined
    const onMove = (event: PointerEvent) => {
      const current = pending.current
      if (!current || event.pointerId !== current.pointerId) return
      pointer.current = { x: event.clientX, y: event.clientY }
      const distance = Math.hypot(event.clientX - current.startX, event.clientY - current.startY)
      if (!dragging.current) {
        if (current.pointerType === 'touch') {
          // Moving before the long press means the person is scrolling.
          if (distance > TOUCH_SLOP) finish()
          return
        }
        if (distance > MOUSE_SLOP) start()
        return
      }
      moveGhost()
      const target = targetAt(event.clientX, event.clientY, current.cardId)
      const previous = dragging.current.target
      if (
        target?.columnKey !== previous?.columnKey ||
        target?.laneKey !== previous?.laneKey ||
        target?.index !== previous?.index
      ) {
        dragging.current = { ...dragging.current, target }
        setDrag(dragging.current)
      }
    }
    const onUp = (event: PointerEvent) => {
      const current = pending.current
      if (!current || event.pointerId !== current.pointerId) return
      const state = dragging.current
      if (state) {
        suppressClick.current = Date.now()
        if (state.target && event.type === 'pointerup') onDropRef.current(state.cardId, state.target)
      }
      finish()
    }
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape' && dragging.current) {
        event.preventDefault()
        suppressClick.current = Date.now()
        finish()
      }
    }
    // Once a card is held, a finger moves it instead of scrolling the page.
    const onTouchMove = (event: TouchEvent) => {
      if (dragging.current) event.preventDefault()
    }
    window.addEventListener('pointermove', onMove)
    window.addEventListener('pointerup', onUp)
    window.addEventListener('pointercancel', onUp)
    window.addEventListener('keydown', onKey)
    document.addEventListener('touchmove', onTouchMove, { passive: false })
    return () => {
      window.removeEventListener('pointermove', onMove)
      window.removeEventListener('pointerup', onUp)
      window.removeEventListener('pointercancel', onUp)
      window.removeEventListener('keydown', onKey)
      document.removeEventListener('touchmove', onTouchMove)
    }
  }, [enabled, finish, moveGhost, start])

  // Scroll the board sideways near its edges, and up and down whichever
  // scrolls that way: the board on a wide screen, the page on a phone.
  const active = drag !== null
  useEffect(() => {
    if (!active) return undefined
    let frame = 0
    const tick = () => {
      const { x, y } = pointer.current
      const board = scrollerRef.current
      if (board) {
        const rect = board.getBoundingClientRect()
        if (x < rect.left + EDGE) board.scrollLeft -= SPEED
        else if (x > rect.right - EDGE) board.scrollLeft += SPEED
      }
      const vertical = board && board.scrollHeight > board.clientHeight ? board : (board?.closest<HTMLElement>('#main-workspace') ?? null)
      if (vertical) {
        const rect = vertical.getBoundingClientRect()
        if (y < rect.top + EDGE) vertical.scrollTop -= SPEED
        else if (y > rect.bottom - EDGE) vertical.scrollTop += SPEED
      }
      frame = window.requestAnimationFrame(tick)
    }
    frame = window.requestAnimationFrame(tick)
    return () => window.cancelAnimationFrame(frame)
  }, [active, scrollerRef])

  const onPointerDown = useCallback(
    (event: ReactPointerEvent<HTMLElement>, cardId: string) => {
      if (!enabled || event.button !== 0 || pending.current) return
      if ((event.target as HTMLElement).closest('button, a, input, label, select, textarea')) {
        // Links and buttons inside a card still start a drag with a mouse, but not a tap.
        if (event.pointerType !== 'mouse' || !(event.target as HTMLElement).closest('a')) return
      }
      const rect = event.currentTarget.getBoundingClientRect()
      pointer.current = { x: event.clientX, y: event.clientY }
      pending.current = {
        cardId,
        pointerId: event.pointerId,
        pointerType: event.pointerType,
        startX: event.clientX,
        startY: event.clientY,
        offsetX: event.clientX - rect.left,
        offsetY: event.clientY - rect.top,
        width: rect.width,
        timer: event.pointerType === 'touch' ? window.setTimeout(start, LONG_PRESS) : undefined,
      }
    },
    [enabled, start],
  )

  /** True right after a drag, so the click that ends it doesn't open the card. */
  const justDragged = useCallback(() => Date.now() - suppressClick.current < 300, [])

  return { drag, onPointerDown, justDragged }
}
