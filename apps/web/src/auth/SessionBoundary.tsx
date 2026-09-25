import { useEffect, useRef } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Navigate, Outlet, useLocation } from 'react-router-dom'

import { BrandMark } from './AuthLayout'

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
      <main className="aw-boot" aria-busy="true">
        <BrandMark />
        <p>Opening your workspace…</p>
      </main>
    )
  }

  if (session.isError) {
    return (
      <main className="aw-boot">
        <BrandMark />
        <h1>Workspace unavailable</h1>
        <p>We couldn’t confirm your session. Check your connection and try again.</p>
        <button className="aw-btn aw-btn--primary aw-btn--md" onClick={() => void session.refetch()} type="button">
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
