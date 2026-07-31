import { useEffect, useRef } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Navigate, Outlet, useLocation } from 'react-router-dom'

import {
  AuthenticatedSessionProvider,
  clearAuthenticatedQueryCache,
  isAuthenticatedSession,
  useSession,
} from './session'
import { RealtimeProvider } from '../realtime/RealtimeProvider'

export function SessionBoundary() {
  const session = useSession()
  const queryClient = useQueryClient()
  const location = useLocation()
  const signedOutCacheCleared = useRef(false)

  useEffect(() => {
    const requireAuthentication = () => {
      signedOutCacheCleared.current = true
      clearAuthenticatedQueryCache(queryClient)
    }
    window.addEventListener(
      'agent-workspace:authentication-required',
      requireAuthentication,
    )
    return () =>
      window.removeEventListener(
        'agent-workspace:authentication-required',
        requireAuthentication,
      )
  }, [queryClient])

  useEffect(() => {
    if (!session.isSuccess) return
    if (isAuthenticatedSession(session.data)) {
      signedOutCacheCleared.current = false
      return
    }
    if (signedOutCacheCleared.current) return
    signedOutCacheCleared.current = true
    clearAuthenticatedQueryCache(queryClient)
  }, [queryClient, session.data, session.isSuccess])

  if (session.isPending) {
    return (
      <main className="session-state" aria-busy="true">
        <div className="brand-mark" aria-hidden="true">
          A
        </div>
        <p>Opening workspace…</p>
      </main>
    )
  }

  if (session.isError) {
    return (
      <main className="session-state">
        <h1>Workspace unavailable</h1>
        <p>The application could not confirm your session.</p>
        <button onClick={() => void session.refetch()} type="button">
          Try again
        </button>
      </main>
    )
  }

  if (!isAuthenticatedSession(session.data)) {
    const returnTo = `${location.pathname}${location.search}${location.hash}`
    return (
      <Navigate
        replace
        to={`/sign-in?returnTo=${encodeURIComponent(returnTo)}`}
      />
    )
  }

  return (
    <AuthenticatedSessionProvider session={session.data}>
      <RealtimeProvider>
        <Outlet />
      </RealtimeProvider>
    </AuthenticatedSessionProvider>
  )
}
