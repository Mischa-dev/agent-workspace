import { useLayoutEffect, type RefObject } from 'react'

/**
 * While mounted, remember what had focus before; on unmount, if focus was
 * inside `ref` (or fell to the page), put it back there.
 */
export function useReturnFocus(ref: RefObject<HTMLElement | null>) {
  useLayoutEffect(() => {
    const before = document.activeElement instanceof HTMLElement ? document.activeElement : null
    const element = ref.current
    return () => {
      const active = document.activeElement
      const lost = !active || active === document.body || (element !== null && element.contains(active))
      if (lost && before?.isConnected) before.focus()
    }
  }, [ref])
}
