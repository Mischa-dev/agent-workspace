import { createContext, useContext } from 'react'

/** Phone widths, where the rail and sidebar become one drawer. Matches shell.css. */
export const MOBILE_QUERY = '(max-width: 860px)'

export function isMobileWidth() {
  return typeof window.matchMedia === 'function' && window.matchMedia(MOBILE_QUERY).matches
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
