import { createContext, useContext, useSyncExternalStore } from 'react'

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
