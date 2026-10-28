import { useRef, useState, type KeyboardEvent, type PointerEvent } from 'react'

import { PANEL_WIDTHS, SIDEBAR_WIDTHS, type WidthLimits } from '../theme/preferences'
import { useTheme } from '../theme/themeContext'

type Edge = {
  label: string
  limits: WidthLimits
  /** The variable on <html> the width lives in, as the theme sets it. */
  variable: string
  /** What the edge resizes. */
  controls: string
  /** Dragging right widens the sidebar and narrows the panel. */
  direction: 1 | -1
  /** The most of the window it takes; styles/shell.css holds the same share. */
  share: number
}

const EDGES: Record<'sidebar' | 'panel', Edge> = {
  sidebar: { label: 'Sidebar width', limits: SIDEBAR_WIDTHS, variable: '--sidebar-width', controls: 'project-sidebar', direction: 1, share: 0.25 },
  panel: { label: 'Side panel width', limits: PANEL_WIDTHS, variable: '--panel-width', controls: 'context-region', direction: -1, share: 0.4 },
}

/** The arrow keys move an edge this far, and four times as far with Shift. */
const KEY_STEP = 16

/** As wide as the edge goes in this window, so the page between never disappears. */
function widest({ limits, share }: Edge) {
  return Math.max(limits.standard, Math.min(limits.max, Math.round(window.innerWidth * share)))
}

/**
 * The edge of the sidebar or the side panel: drag it to resize, double-click
 * to go back to the standard width, or focus it and use the arrow keys (Home
 * and End for the narrowest and widest, Enter to reset). The width is saved
 * with your appearance, so it follows you to other devices. Hidden on
 * phones, where both are drawers.
 */
export function ResizeEdge({ edge }: { edge: keyof typeof EDGES }) {
  const theme = useTheme()
  const spec = EDGES[edge]
  const saved = edge === 'sidebar' ? theme.sidebarWidth : theme.panelWidth
  const save = edge === 'sidebar' ? theme.setSidebarWidth : theme.setPanelWidth
  const current = Math.min(saved ?? spec.limits.standard, widest(spec))
  const [dragging, setDragging] = useState(false)
  const drag = useRef<{ pointer: number; x: number; width: number; moved: number | null } | null>(null)

  const clamp = (width: number) => Math.min(widest(spec), Math.max(spec.limits.min, Math.round(width)))

  const start = (event: PointerEvent<HTMLDivElement>) => {
    if (event.button !== 0) return
    event.preventDefault()
    const width = document.getElementById(spec.controls)?.getBoundingClientRect().width || current
    drag.current = { pointer: event.pointerId, x: event.clientX, width, moved: null }
    event.currentTarget.setPointerCapture(event.pointerId)
    document.documentElement.dataset.resizing = 'true'
    setDragging(true)
  }

  const move = (event: PointerEvent<HTMLDivElement>) => {
    const active = drag.current
    if (!active || event.pointerId !== active.pointer) return
    const width = clamp(active.width + spec.direction * (event.clientX - active.x))
    active.moved = width
    // Straight onto the page while dragging; saved once, when it ends.
    document.documentElement.style.setProperty(spec.variable, `${width}px`)
  }

  const finish = () => {
    const active = drag.current
    if (!active) return
    drag.current = null
    delete document.documentElement.dataset.resizing
    setDragging(false)
    if (active.moved !== null) save(active.moved)
  }

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    const step = (event.shiftKey ? 4 : 1) * KEY_STEP
    let next: number | null
    if (event.key === 'ArrowRight') next = clamp(current + spec.direction * step)
    else if (event.key === 'ArrowLeft') next = clamp(current - spec.direction * step)
    else if (event.key === 'Home') next = spec.limits.min
    else if (event.key === 'End') next = widest(spec)
    else if (event.key === 'Enter') next = null
    else return
    event.preventDefault()
    save(next)
  }

  return (
    <div
      aria-controls={spec.controls}
      aria-label={spec.label}
      aria-orientation="vertical"
      aria-valuemax={widest(spec)}
      aria-valuemin={spec.limits.min}
      aria-valuenow={current}
      className={`aw-resize aw-resize--${edge}${dragging ? ' is-dragging' : ''}`}
      onDoubleClick={() => save(null)}
      onKeyDown={onKeyDown}
      onLostPointerCapture={finish}
      onPointerCancel={finish}
      onPointerDown={start}
      onPointerMove={move}
      onPointerUp={finish}
      role="separator"
      tabIndex={0}
      title="Drag to resize, double-click to reset"
    />
  )
}
