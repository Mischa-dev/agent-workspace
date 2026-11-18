import { createContext, useContext, useEffect, useRef, useSyncExternalStore } from 'react'

/** Phone widths, where the rail and sidebar become one drawer. Matches shell.css. */
export const MOBILE_QUERY = '(max-width: 860px)'

export function isMobileWidth() {
  return typeof window.matchMedia === 'function' && window.matchMedia(MOBILE_QUERY).matches
}

function subscribeWidth(onChange: () => void) {
  if (typeof window.matchMedia !== 'function') return () => undefined
  const query = window.matchMedia(MOBILE_QUERY)
  query.addEventListener?.('change', onChange)
  return () => query.removeEventListener?.('change', onChange)
}

/** Whether the window is phone width now, following it as it changes. */
export function useMobileWidth() {
  return useSyncExternalStore(subscribeWidth, isMobileWidth, () => false)
}

/**
 * A drawer or sheet that the phone's back button closes (W1.29): opening it
 * at phone width adds a history entry at the same address, and going back
 * closes it instead of leaving the page. Closing it any other way takes the
 * entry back out, unless the person went somewhere else from it.
 */
export function useBackCloses(open: boolean, close: () => void) {
  const closeRef = useRef(close)
  useEffect(() => {
    closeRef.current = close
  }, [close])
  useEffect(() => {
    if (!open || !isMobileWidth()) return undefined
    const marker = `sheet-${Date.now()}-${Math.random().toString(36).slice(2)}`
    const state: unknown = window.history.state
    window.history.pushState({ ...(state && typeof state === 'object' ? state : {}), awSheet: marker }, '')
    let entered = true
    const onPop = () => {
      entered = false
      closeRef.current()
    }
    window.addEventListener('popstate', onPop)
    return () => {
      window.removeEventListener('popstate', onPop)
      const current: unknown = window.history.state
      if (entered && current && typeof current === 'object' && (current as { awSheet?: string }).awSheet === marker) window.history.back()
    }
  }, [open])
}

type ShellHeader = {
  /** Opens the navigation drawer, returning focus to `trigger` when it closes. */
  openNavigation: (trigger: HTMLElement | null) => void
  navigationOpen: boolean
}

export const ShellHeaderContext = createContext<ShellHeader | null>(null)

/** Lets a conversation header carry the phone menu button, since it replaces the shell's own bar there. */
export function useShellHeader() {
  return useContext(ShellHeaderContext)
}
