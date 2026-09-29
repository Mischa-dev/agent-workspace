import { createContext, useContext, useState } from 'react'

/** Phone widths, where the rail and sidebar become one drawer. Matches shell.css. */
export const MOBILE_QUERY = '(max-width: 860px)'

export function isMobileWidth() {
  return typeof window.matchMedia === 'function' && window.matchMedia(MOBILE_QUERY).matches
}

export type MobileNavigation = {
  open: boolean
  setOpen: (open: boolean) => void
  /**
   * Keeps the drawer open through the next route change. Choosing a Project or
   * Messages from the drawer swaps the sidebar beside the rail instead of closing,
   * so the next tap can pick a channel or conversation.
   */
  keepOpenOnNextNavigation: () => void
}

export const MobileNavigationContext = createContext<MobileNavigation | null>(null)

/** The shared drawer state, or a local one where no provider is mounted (tests, isolated pages). */
export function useMobileNavigation(): MobileNavigation {
  const shared = useContext(MobileNavigationContext)
  const [open, setOpen] = useState(false)
  return shared ?? { open, setOpen, keepOpenOnNextNavigation: () => undefined }
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
