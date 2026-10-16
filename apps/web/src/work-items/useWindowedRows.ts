import { useCallback, useEffect, useLayoutEffect, useMemo, useState, type RefObject } from 'react'

/** Start offset of every row, plus the total height at the end. */
export function prefixOffsets(heights: readonly number[]) {
  const offsets = new Array<number>(heights.length + 1)
  offsets[0] = 0
  for (let index = 0; index < heights.length; index += 1) offsets[index + 1] = offsets[index] + heights[index]
  return offsets
}

/** The first row whose bottom edge is below `y`. */
export function rowAt(offsets: readonly number[], y: number) {
  let low = 0
  let high = offsets.length - 2
  if (high < 0) return 0
  while (low < high) {
    const middle = (low + high) >> 1
    if (offsets[middle + 1] <= y) low = middle + 1
    else high = middle
  }
  return low
}

/** The rows that cover a viewport, with `overscan` extra on each side. */
export function windowFor(offsets: readonly number[], top: number, height: number, overscan: number) {
  const count = offsets.length - 1
  if (count <= 0) return { start: 0, end: 0 }
  const first = rowAt(offsets, Math.max(0, top))
  const last = rowAt(offsets, Math.max(0, top + height))
  return { start: Math.max(0, first - overscan), end: Math.min(count, last + 1 + overscan) }
}

function scrollParent(element: HTMLElement | null): HTMLElement | null {
  let node = element?.parentElement ?? null
  while (node) {
    const overflow = getComputedStyle(node).overflowY
    if (overflow === 'auto' || overflow === 'scroll') return node
    node = node.parentElement
  }
  return null
}

/**
 * Draws only the rows in view. The list scrolls with its page (the nearest
 * scrolling ancestor), so a long list keeps the page header and filter bar
 * above it. Row heights are known up front, which keeps scrolling exact.
 */
export function useWindowedRows(listRef: RefObject<HTMLElement | null>, heights: readonly number[], overscan = 12) {
  const offsets = useMemo(() => prefixOffsets(heights), [heights])
  const [view, setView] = useState({ top: 0, height: typeof window === 'undefined' ? 800 : window.innerHeight })
  const [scroller, setScroller] = useState<HTMLElement | null>(null)

  useLayoutEffect(() => {
    setScroller(scrollParent(listRef.current))
  }, [listRef])

  const measure = useCallback(() => {
    const list = listRef.current
    if (!list) return
    const frame = scroller?.getBoundingClientRect() ?? { top: 0, height: window.innerHeight }
    const viewportHeight = scroller?.clientHeight || window.innerHeight
    const top = frame.top - list.getBoundingClientRect().top
    setView((current) => (current.top === top && current.height === viewportHeight ? current : { top, height: viewportHeight }))
  }, [listRef, scroller])

  useEffect(() => {
    let frame = 0
    const schedule = () => {
      if (frame) return
      frame = window.requestAnimationFrame(() => {
        frame = 0
        measure()
      })
    }
    measure()
    const target: HTMLElement | Window = scroller ?? window
    target.addEventListener('scroll', schedule, { passive: true })
    window.addEventListener('resize', schedule)
    return () => {
      if (frame) window.cancelAnimationFrame(frame)
      target.removeEventListener('scroll', schedule)
      window.removeEventListener('resize', schedule)
    }
  }, [measure, scroller])

  const range = windowFor(offsets, view.top, view.height, overscan)

  /** Scrolls just enough to show a row, keeping `margin` pixels around it (for a sticky header). */
  const scrollToIndex = useCallback(
    (index: number, margin = 40) => {
      const list = listRef.current
      if (!list || index < 0 || index >= offsets.length - 1) return
      const parent = scrollParent(list)
      const viewportHeight = parent?.clientHeight || window.innerHeight
      const frameTop = parent?.getBoundingClientRect().top ?? 0
      const listTop = list.getBoundingClientRect().top - frameTop
      const rowTop = listTop + offsets[index]
      const rowBottom = listTop + offsets[index + 1]
      let delta = 0
      if (rowTop < margin) delta = rowTop - margin
      else if (rowBottom > viewportHeight - 8) delta = rowBottom - viewportHeight + 8
      if (!delta) return
      if (parent) parent.scrollTop += delta
      else window.scrollBy(0, delta)
      measure()
    },
    [listRef, measure, offsets],
  )

  return {
    start: range.start,
    end: range.end,
    before: offsets[range.start] ?? 0,
    after: (offsets[offsets.length - 1] ?? 0) - (offsets[range.end] ?? 0),
    scrollToIndex,
  }
}
