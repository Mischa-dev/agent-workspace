import { useCallback, useEffect, useRef, useState, type FormEvent } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useLocation, useNavigate } from 'react-router-dom'

import { isApiError } from '../api/errors'
import { Banner, Button, TextField } from '../ui'
import { startWorkOs, workOsStatus } from './workos'
import { AuthLayout } from './AuthLayout'
import {
  installAuthenticatedSession,
  isAuthenticatedSession,
  type SessionResponse,
  useSession,
} from './session'
import { isTwoFactorChallenge, login, loginTwoFactor } from './login'

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
  // Set once the password was right and the account asks for a second step.
  const [challenge, setChallenge] = useState<string | null>(null)
  const [code, setCode] = useState('')
  const returnTo = safeReturnTo(location.search)
  const goBack = useCallback(() => {
    // An app's connect screen is served by the server, not this app.
    if (returnTo.startsWith('/oauth/')) window.location.replace(returnTo)
    else navigate(returnTo, { replace: true })
  }, [navigate, returnTo])

  useEffect(() => {
    if (isAuthenticatedSession(session.data)) goBack()
  }, [goBack, session.data])

  useEffect(() => {
    if (error) errorRef.current?.focus()
  }, [error])

  const workOsSignIn = async (signup = false, provider?: 'GoogleOAuth') => {
    setSubmitting(true)
    setError(null)
    try { await startWorkOs(signup ? { username: loginName.trim(), provider } : { provider }) }
    catch { setError('Could not open secure sign-in. Try again.'); setSubmitting(false) }
  }

  const finish = (nextSession: SessionResponse) => {
    if (!isAuthenticatedSession(nextSession)) {
      throw new Error('Sign-in completed without an authenticated session.')
    }
    installAuthenticatedSession(queryClient, nextSession)
    goBack()
  }

  const describeFailure = (reason: unknown) => {
    if (isApiError(reason) && reason.code === 'rate_limited') return 'Too many attempts. Wait a moment, then try again.'
    return isApiError(reason) ? reason.message : 'Sign-in didn’t go through. Try again.'
  }

  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!loginName.trim() || password.length < 12) return
    setSubmitting(true)
    setError(null)
    try {
      const response = await login(loginName.trim(), password)
      setPassword('')
      if (isTwoFactorChallenge(response)) {
        setCode('')
        setChallenge(response.challenge)
        return
      }
      finish(response)
    } catch (reason) {
      setPassword('')
      setError(describeFailure(reason))
    } finally {
      setSubmitting(false)
    }
  }

  const submitCode = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    const trimmed = code.trim()
    if (!challenge || !trimmed) return
    setSubmitting(true)
    setError(null)
    try {
      finish(await loginTwoFactor(challenge, trimmed))
    } catch (reason) {
      setCode('')
      if (isApiError(reason) && reason.status === 404) {
        // Too many wrong codes, or five minutes went by: start again.
        setChallenge(null)
        setError('That sign-in expired. Enter your password again.')
      } else if (isApiError(reason) && reason.code === 'invalid_credentials') {
        setError('That code didn’t work. Check your authenticator app and try again.')
      } else {
        setError(describeFailure(reason))
      }
    } finally {
      setSubmitting(false)
    }
  }

  if (challenge) {
    return (
      <AuthLayout
        intro="Enter the 6-digit code from your authenticator app, or one of your recovery codes."
        title="Two-factor authentication"
        titleId="two-factor-heading"
      >
        {error && (
          <div className="aw-auth__error" ref={errorRef} role="alert" tabIndex={-1}>
            <Banner tone="danger">{error}</Banner>
          </div>
        )}
        <form className="aw-auth__form" onSubmit={submitCode}>
          <TextField
            autoComplete="one-time-code"
            autoFocus
            disabled={submitting}
            hint="Recovery codes look like xxxxx-xxxxx and work once."
            id="two-factor-code"
            label="Code"
            maxLength={32}
            mono
            onChange={(event) => setCode(event.target.value)}
            required
            spellCheck={false}
            value={code}
          />
          <Button
            busy={submitting}
            className="aw-auth__submit"
            disabled={!code.trim()}
            size="lg"
            type="submit"
            variant="primary"
          >
            {submitting ? 'Checking…' : 'Verify'}
          </Button>
          <Button
            disabled={submitting}
            onClick={() => {
              setChallenge(null)
              setCode('')
              setError(null)
            }}
            variant="ghost"
          >
            Use a different account
          </Button>
        </form>
      </AuthLayout>
    )
  }

  return (
    <AuthLayout
      footer={
        <>
          Existing account? You can link WorkOS from Account settings after signing in.
          {' '}Forgot your password? Whoever runs this server can reset it for you.
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
