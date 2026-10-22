import { useLayoutEffect, useRef, useState, type CSSProperties, type ReactNode } from 'react'

import { clamp, opening } from '../motion'

const sizes = new Map<string, { w: number; h: number }>()

/**
 * One thing turning into another, as a live page updates in place: while k
 * runs from 0 to 1 both sit in one cell, the old fading out as the new fades
 * in, and the cell eases from the old one's size to the new one's, so nothing
 * beside or under it jumps when the swap ends. Before that only the old one is
 * drawn, and after it only the new one. `inline` sizes the cell by width (a
 * chip in a row) instead of height (a line or a block), and `align` keeps
 * an inline pair on the cell's start or end.
 */
export function Swap({
  align = 'start',
  from,
  id,
  inline = false,
  k,
  to,
}: {
  align?: 'start' | 'end'
  from: ReactNode
  id: string
  inline?: boolean
  k: number
  to: ReactNode
}) {
  const before = useRef<HTMLDivElement>(null)
  const after = useRef<HTMLDivElement>(null)
  const [, bump] = useState(0)
  const swapping = k > 0 && k < 1
  useLayoutEffect(() => {
    if (!swapping) return
    let changed = false
    for (const [key, element] of [
      [`${id}:from`, before.current],
      [`${id}:to`, after.current],
    ] as const) {
      if (!element) continue
      const style = getComputedStyle(element)
      const size = { w: parseFloat(style.width), h: parseFloat(style.height) }
      const known = sizes.get(key)
      if (!known || known.w !== size.w || known.h !== size.h) {
        sizes.set(key, size)
        changed = true
      }
    }
    if (changed) bump((n) => n + 1)
  })
  if (k <= 0) return <>{from}</>
  if (k >= 1) return <>{to}</>
  const a = sizes.get(`${id}:from`)
  const b = sizes.get(`${id}:to`)
  const grow = opening(k)
  const size = a && b ? (inline ? { width: a.w + (b.w - a.w) * grow } : { height: a.h + (b.h - a.h) * grow }) : {}
  const cell: CSSProperties = {
    display: inline ? 'inline-grid' : 'grid',
    justifyItems: inline ? align : 'stretch',
    verticalAlign: 'middle',
    ...size,
  }
  const item: CSSProperties = { gridArea: '1 / 1', alignSelf: 'start', ...(inline ? { width: 'max-content' } : {}) }
  return (
    <span style={cell}>
      <span ref={before} style={{ ...item, display: inline ? 'inline-flex' : 'block', opacity: clamp(1 - k * 1.5) }}>
        {from}
      </span>
      <span ref={after} style={{ ...item, display: inline ? 'inline-flex' : 'block', opacity: clamp((k - 0.15) / 0.85) }}>
        {to}
      </span>
    </span>
  )
}
