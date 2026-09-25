import { useEffect, useRef, useState, type FormEvent } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { useLocation, useNavigate } from 'react-router-dom'

import { isApiError } from '../api/errors'
import { Banner, Button, TextField } from '../ui'
import { AuthLayout } from './AuthLayout'
import { installAuthenticatedSession, isAuthenticatedSession, login, useSession } from './session'

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
        setError(isApiError(reason) ? reason.message : 'Sign-in didn’t go through. Try again.')
      }
    } finally {
      setSubmitting(false)
    }
  }

  return (
    <AuthLayout
      footer={
        <>
          New host? Create the first account on the server with <code>bootstrap-admin</code>. There’s no sign-up page.
        </>
      }
      intro="Welcome back. Your conversations and agents are waiting."
      title="Sign in"
      titleId="sign-in-heading"
    >
      {error && (
        <div className="aw-auth__error" ref={errorRef} role="alert" tabIndex={-1}>
          <Banner tone="danger">{error}</Banner>
        </div>
      )}
      <form className="aw-auth__form" onSubmit={submit}>
        <TextField
          autoComplete="username"
          autoFocus
          disabled={submitting}
          id="login-name"
          label="Login name"
          maxLength={64}
          onChange={(event) => setLoginName(event.target.value)}
          required
          value={loginName}
        />
        <TextField
          autoComplete="current-password"
          disabled={submitting}
          id="password"
          label="Password"
          minLength={12}
          onChange={(event) => setPassword(event.target.value)}
          required
          type="password"
          value={password}
        />
        <Button
          busy={submitting}
          className="aw-auth__submit"
          disabled={!loginName.trim() || password.length < 12}
          size="lg"
          type="submit"
          variant="primary"
        >
          {submitting ? 'Signing in…' : 'Sign in'}
        </Button>
      </form>
    </AuthLayout>
  )
}
