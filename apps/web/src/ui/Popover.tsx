import { useEffect, useId, useLayoutEffect, useRef, useState, type ReactNode, type RefObject } from 'react'
import { createPortal } from 'react-dom'

import './pickers.css'
import { popoverPosition, type PopoverPlacement } from './popoverModel'

const fields = 'input:not([disabled]), textarea:not([disabled]), select:not([disabled])'
const focusable = `${fields}, button:not([disabled]), a[href], [tabindex]:not([tabindex="-1"])`

/**
 * A small floating panel tied to an anchor, such as a picker. It isn't modal:
 * Escape or a click outside closes it, and focus goes back to the anchor.
 * On phones it docks to the bottom of the screen as a sheet.
 */
export function Popover({
  anchor,
  children,
  className,
  initialFocus,
  label,
  onClose,
  open,
  placement = 'bottom-start',
  returnFocus = true,
  role = 'dialog',
  width,
}: {
  anchor: HTMLElement | null
  children: ReactNode
  className?: string
  /** What takes focus on open; defaults to the first field inside, else the first control. */
  initialFocus?: RefObject<HTMLElement | null>
  label: string
  onClose: () => void
  open: boolean
  placement?: PopoverPlacement
  /** Put focus back on the anchor when it closes. */
  returnFocus?: boolean
  role?: 'dialog' | 'region'
  width?: number
}) {
  const panel = useRef<HTMLDivElement>(null)
  const [position, setPosition] = useState<{ top: number; left: number } | null>(null)
  const closeRef = useRef(onClose)
  const id = useId()
  useEffect(() => {
    closeRef.current = onClose
  }, [onClose])

  useLayoutEffect(() => {
    if (!open || !anchor) return undefined
    const update = () => {
      const element = panel.current
      if (!element) return
      setPosition(
        popoverPosition(
          anchor.getBoundingClientRect(),
          { width: element.offsetWidth, height: element.offsetHeight },
          placement,
          { width: window.innerWidth, height: window.innerHeight },
        ),
      )
    }
    update()
    const frame = window.requestAnimationFrame(update)
    const observer = typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(update)
    if (panel.current) observer?.observe(panel.current)
    window.addEventListener('resize', update)
    window.addEventListener('scroll', update, true)
    return () => {
      window.cancelAnimationFrame(frame)
      observer?.disconnect()
      window.removeEventListener('resize', update)
      window.removeEventListener('scroll', update, true)
    }
  }, [anchor, open, placement])

  // Focus moves in once per opening, when the panel has its place: until then
  // it's hidden, and a hidden panel can't take focus.
  const placed = position !== null
  useEffect(() => {
    if (!open || !placed) return
    const target = initialFocus?.current ?? panel.current?.querySelector<HTMLElement>(fields) ?? panel.current?.querySelector<HTMLElement>(focusable) ?? panel.current
    target?.focus({ preventScroll: true })
    // The focus target is read once, as the panel shows.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, placed])

  useEffect(() => {
    if (!open) return undefined
    const onPointer = (event: PointerEvent) => {
      const node = event.target as Node
      if (panel.current?.contains(node) || anchor?.contains(node)) return
      closeRef.current()
    }
    document.addEventListener('pointerdown', onPointer)
    return () => {
      document.removeEventListener('pointerdown', onPointer)
      if (!returnFocus) return
      // Back to the anchor only when closing left focus nowhere: a click
      // into another field keeps it there.
      window.requestAnimationFrame(() => {
        const active = document.activeElement
        if (anchor?.isConnected && (!active || active === document.body)) anchor.focus({ preventScroll: true })
      })
    }
    // The anchor is read once per opening.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open])

  if (!open) return null
  return createPortal(
    <div
      aria-label={label}
      className={['aw-popover', className].filter(Boolean).join(' ')}
      data-region="content"
      id={id}
      onKeyDown={(event) => {
        if (event.key === 'Escape') {
          event.preventDefault()
          event.stopPropagation()
          closeRef.current()
        }
      }}
      ref={panel}
      role={role}
      style={{ ...(position ? { top: position.top, left: position.left } : { visibility: 'hidden', top: 0, left: 0 }), ...(width ? { width } : {}) }}
      tabIndex={-1}
    >
      {children}
    </div>,
    document.body,
  )
}
