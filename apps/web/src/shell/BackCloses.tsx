import { useEffect, useRef } from 'react'

import { isMobileWidth } from './mobileNavigation'

/**
 * A drawer or sheet that the phone's back button closes (W1.29): opening it
 * at phone width adds a history entry at the same address, and going back
 * closes it instead of leaving the page. Closing it any other way takes the
 * entry back out, unless the person went somewhere else from it.
 */
function useBackCloses(open: boolean, close: () => void) {
  const closeRef = useRef(close)
  useEffect(() => {
    closeRef.current = close
  }, [close])
  // The entry this sheet added, and a going-back that waits a tick: an effect
  // that runs again straight away (React's strict mode, or mounting while
  // already open) keeps the entry instead of leaving and re-adding it.
  const entry = useRef<{ marker: string; leaving: number | null } | null>(null)
  useEffect(() => {
    if (!open || !isMobileWidth()) return undefined
    const state: unknown = window.history.state
    const ours = entry.current && markerOf(state) === entry.current.marker ? entry.current : null
    if (ours?.leaving != null) window.clearTimeout(ours.leaving)
    const marker = ours?.marker ?? `sheet-${Date.now()}-${Math.random().toString(36).slice(2)}`
    if (!ours) window.history.pushState({ ...(state && typeof state === 'object' ? state : {}), awSheet: marker }, '')
    const current = { marker, leaving: null as number | null }
    entry.current = current
    let entered = true
    const onPop = () => {
      entered = false
      closeRef.current()
    }
    window.addEventListener('popstate', onPop)
    return () => {
      window.removeEventListener('popstate', onPop)
      if (!entered) return
      current.leaving = window.setTimeout(() => {
        current.leaving = null
        if (markerOf(window.history.state) === marker) window.history.back()
      }, 0)
    }
  }, [open])
}

function markerOf(state: unknown): string | undefined {
  return state && typeof state === 'object' ? (state as { awSheet?: string }).awSheet : undefined
}

/** The hook as a component, so the shell loads it after the first paint (shellLazy.ts). */
export default function BackCloses({ open, close }: { open: boolean; close: () => void }) {
  useBackCloses(open, close)
  return null
}
