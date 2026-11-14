import { useCallback, useMemo, useRef, useState } from 'react'
import { Outlet } from 'react-router-dom'

import { useAuthenticatedSession } from '../auth/session'
import { ShellFrame, type ShellPresence } from './AppShell'
import { mergePresence, samePresence, ShellPresenceContext, type ShellRegistrar } from './shellStore'

/**
 * The persistent shell (W1.11): the rail, the sidebar and the panel, mounted
 * once around every shell page. Pages say what they need beside them
 * (`AppShell`'s props, or `useShellSpace` and `useShellPanel`); moving from
 * page to page changes what the shell draws without remounting it, so the
 * sidebar keeps its place and scroll.
 */
export function ShellLayout() {
  const session = useAuthenticatedSession()
  const registrants = useRef(new Map<string, ShellPresence>())
  const [presence, setPresence] = useState<ShellPresence>({})
  const register = useCallback<ShellRegistrar>((id, next) => {
    if (next) registrants.current.set(id, next)
    else registrants.current.delete(id)
    const merged = mergePresence(registrants.current.values())
    setPresence((current) => (samePresence(current, merged) ? current : merged))
  }, [])
  const outlet = useMemo(() => <Outlet />, [])
  return (
    <ShellPresenceContext.Provider value={register}>
      <ShellFrame {...presence} user={session.user}>
        {outlet}
      </ShellFrame>
    </ShellPresenceContext.Provider>
  )
}
