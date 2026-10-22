import { useLayoutEffect, useRef, useState } from 'react'

export type Box = { x: number; y: number; w: number; h: number }

const cache = new Map<string, Box>()

/** Where an element sits inside `root`, in root's own (unscaled) pixels, ignoring transforms. */
function boxWithin(element: HTMLElement, root: HTMLElement): Box | null {
  let x = 0
  let y = 0
  let node: HTMLElement | null = element
  while (node && node !== root) {
    x += node.offsetLeft - (node.offsetParent instanceof HTMLElement ? node.offsetParent.scrollLeft : 0)
    y += node.offsetTop - (node.offsetParent instanceof HTMLElement ? node.offsetParent.scrollTop : 0)
    const parent: Element | null = node.offsetParent
    node = parent instanceof HTMLElement ? parent : null
  }
  if (node !== root) return null
  return { x, y, w: element.offsetWidth, h: element.offsetHeight }
}

/**
 * Measures elements inside a scene's window so the pointer and carried cards
 * can go to them. It measures after every render and re-renders at once when
 * something moved, so even the first frame of a render worker is right.
 */
export function useBoxes(scope: string, selectors: Record<string, string>) {
  const root = useRef<HTMLDivElement>(null)
  const [, bump] = useState(0)
  useLayoutEffect(() => {
    const element = root.current
    if (!element) return
    let changed = false
    for (const [name, selector] of Object.entries(selectors)) {
      const target = element.querySelector<HTMLElement>(selector)
      if (!target) continue
      const box = boxWithin(target, element)
      if (!box) continue
      const key = `${scope}:${name}`
      const old = cache.get(key)
      if (!old || old.x !== box.x || old.y !== box.y || old.w !== box.w || old.h !== box.h) {
        cache.set(key, box)
        changed = true
      }
    }
    if (changed) bump((n) => n + 1)
  })
  const box = (name: string): Box => cache.get(`${scope}:${name}`) ?? { x: 0, y: 0, w: 0, h: 0 }
  const center = (name: string, dx = 0, dy = 0) => {
    const b = box(name)
    return { x: b.x + b.w / 2 + dx, y: b.y + b.h / 2 + dy }
  }
  return { root, box, center, ready: (name: string) => cache.has(`${scope}:${name}`) }
}
