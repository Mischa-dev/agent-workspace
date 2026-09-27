import { useEffect, useRef, useState, type FormEvent } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useLocation, useNavigate } from 'react-router-dom'

import { isApiError } from '../api/errors'
import { Banner, Button, TextField } from '../ui'
import { startWorkOs, workOsStatus } from './workos'
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
  const workos = useQuery({ queryKey: ['workos-status'], queryFn: workOsStatus })
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

  const workOsSignIn = async (signup = false, provider?: 'GoogleOAuth') => {
    setSubmitting(true)
    setError(null)
    try { await startWorkOs(signup ? { username: loginName.trim(), provider } : { provider }) }
    catch { setError('Could not open secure sign-in. Try again.'); setSubmitting(false) }
  }

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
          Existing account? You can link WorkOS from Account settings after signing in.
        </>
      }
      intro="Welcome back. Your conversations and agents are waiting."
      title="Sign in"
      titleId="sign-in-heading"
    >
      {new URLSearchParams(location.search).has('workosError') && <Banner tone="danger">WorkOS sign-in could not be completed. If you have an existing account, sign in below and link it in Account settings.</Banner>}
      {workos.data?.googleEnabled && <Button busy={submitting} onClick={() => void workOsSignIn(false, 'GoogleOAuth')} variant="primary">Continue with Google</Button>}
      {workos.data?.enabled && <Button busy={submitting} onClick={() => void workOsSignIn()} variant="primary">Continue with WorkOS</Button>}
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
        {workos.data?.signupEnabled && <Button disabled={submitting || loginName.trim().length < 3} onClick={() => void workOsSignIn(true)}>Create account with this username</Button>}
      </form>
    </AuthLayout>
  )
}
