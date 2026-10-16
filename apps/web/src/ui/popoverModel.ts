export type PopoverPlacement = 'bottom-start' | 'bottom-end' | 'top-start' | 'right-start'

const GAP = 6
const EDGE = 8

/** Fixed coordinates beside an anchor, flipped and clamped to stay in the viewport. */
export function popoverPosition(anchor: DOMRect, size: { width: number; height: number }, placement: PopoverPlacement, viewport: { width: number; height: number }) {
  let top = anchor.bottom + GAP
  let left = anchor.left
  if (placement === 'bottom-end') left = anchor.right - size.width
  if (placement === 'top-start') top = anchor.top - size.height - GAP
  if (placement === 'right-start') {
    left = anchor.right + GAP
    top = anchor.top
  }
  if (placement.startsWith('bottom') && top + size.height > viewport.height - EDGE && anchor.top - size.height - GAP > EDGE) {
    top = anchor.top - size.height - GAP
  }
  if (placement === 'top-start' && top < EDGE) top = anchor.bottom + GAP
  left = Math.max(EDGE, Math.min(left, viewport.width - size.width - EDGE))
  top = Math.max(EDGE, Math.min(top, viewport.height - size.height - EDGE))
  return { top, left }
}

