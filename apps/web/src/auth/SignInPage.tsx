import { FormEvent, useEffect, useRef, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { useLocation, useNavigate } from 'react-router-dom'

import { isApiError } from '../api/errors'
import {
  installAuthenticatedSession,
  isAuthenticatedSession,
  login,
  useSession,
} from './session'
import { AppearanceControl } from '../shell/AppearanceControl'

function safeReturnTo(search: string) {
  const candidate = new URLSearchParams(search).get('returnTo')
  if (!candidate?.startsWith('/') || candidate.startsWith('//')) return '/home'
  try {
    const url = new URL(candidate, window.location.origin)
    if (url.origin !== window.location.origin || url.pathname === '/sign-in') {
      return '/home'
    }
    return `${url.pathname}${url.search}${url.hash}`
  } catch {
    return '/home'
  }
}

export function SignInPage() {
  const session = useSession()
  const queryClient = useQueryClient()
  const navigate = useNavigate()
  const location = useLocation()
  const errorRef = useRef<HTMLDivElement>(null)
  const [loginName, setLoginName] = useState('')
  const [password, setPassword] = useState('')
  const [submitting, setSubmitting] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const returnTo = safeReturnTo(location.search)

  useEffect(() => {
    if (isAuthenticatedSession(session.data)) {
      navigate(returnTo, { replace: true })
    }
  }, [navigate, returnTo, session.data])

  useEffect(() => {
    if (error) errorRef.current?.focus()
  }, [error])

  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!loginName.trim() || password.length < 12) return
    setSubmitting(true)
    setError(null)
    try {
      const nextSession = await login(loginName.trim(), password)
      if (!isAuthenticatedSession(nextSession)) {
        throw new Error('Sign-in completed without an authenticated session.')
      }
      installAuthenticatedSession(queryClient, nextSession)
      setPassword('')
      navigate(returnTo, { replace: true })
    } catch (reason) {
      setPassword('')
      if (isApiError(reason) && reason.code === 'rate_limited') {
        setError('Too many attempts. Wait a moment, then try again.')
      } else {
        setError(
          isApiError(reason)
            ? reason.message
            : 'Sign-in could not be completed. Try again.',
        )
      }
    } finally {
      setSubmitting(false)
    }
  }

  return (
    <main className="sign-in-page">
      <div className="public-appearance">
        <AppearanceControl />
      </div>
      <section aria-labelledby="sign-in-heading" className="sign-in-panel">
        <div className="brand-mark" aria-hidden="true">
          A
        </div>
        <div>
          <span className="section-label">Private workspace</span>
          <h1 id="sign-in-heading">Sign in</h1>
          <p className="sign-in-intro">
            Use the administrator account created on this Project Host.
          </p>
        </div>

        {error && (
          <div
            className="form-error-summary"
            ref={errorRef}
            role="alert"
            tabIndex={-1}
          >
            {error}
          </div>
        )}

        <form onSubmit={submit}>
          <div className="field">
            <label htmlFor="login-name">Login name</label>
            <input
              autoComplete="username"
              autoFocus
              disabled={submitting}
              id="login-name"
              maxLength={64}
              onChange={(event) => setLoginName(event.target.value)}
              required
              value={loginName}
            />
          </div>
          <div className="field">
            <label htmlFor="password">Password</label>
            <input
              autoComplete="current-password"
              disabled={submitting}
              id="password"
              minLength={12}
              onChange={(event) => setPassword(event.target.value)}
              required
              type="password"
              value={password}
            />
          </div>
          <button
            className="primary-button sign-in-submit"
            disabled={submitting || !loginName.trim() || password.length < 12}
            type="submit"
          >
            {submitting ? 'Signing in…' : 'Sign in'}
          </button>
        </form>

        <p className="operator-note">
          New host? The first account is created from the host-local
          <code> bootstrap-admin</code> command. There is no browser setup route.
        </p>
      </section>
    </main>
  )
}
