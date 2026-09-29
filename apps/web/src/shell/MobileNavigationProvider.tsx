import { useMemo, useState, type ReactNode } from 'react'
import { useLocation } from 'react-router-dom'

import { MobileNavigationContext, type MobileNavigation } from './mobileNavigation'

/**
 * Holds the phone navigation drawer above the routes. Every page renders its own
 * AppShell, so without this the drawer would close whenever the route changed.
 */
export function MobileNavigationProvider({ children }: { children: ReactNode }) {
  const location = useLocation()
  const [state, setState] = useState({ open: false, keep: false, key: location.key })
  if (state.key !== location.key) {
    // Any other navigation (a link, the back button) closes the drawer.
    setState({ open: state.open && state.keep, keep: false, key: location.key })
  }
  const value = useMemo<MobileNavigation>(
    () => ({
      open: state.open,
      setOpen: (open) => setState((current) => ({ ...current, open, keep: open && current.keep })),
      keepOpenOnNextNavigation: () => setState((current) => ({ ...current, keep: true })),
    }),
    [state.open],
  )
  return <MobileNavigationContext.Provider value={value}>{children}</MobileNavigationContext.Provider>
}
