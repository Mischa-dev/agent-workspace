import { useCallback, useEffect, useRef, useState, type FormEvent } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Link, useLocation, useNavigate } from 'react-router-dom'

import { requestJson } from '../api/client'
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
  // Whether this server can email a reset link; without email the operator resets it.
  const resetByEmail = useQuery({
    queryKey: ['password-reset-available'],
    queryFn: () => requestJson<{ available: boolean }>('/api/v1/auth/password-reset'),
  })
  const session = useSession()
  const queryClient = useQueryClient()
  const navigate = useNavigate()
  const location = useLocation()
  const errorRef = useRef<HTMLDivElement>(null)
  const [loginName, setLoginName] = useState('')
  const [password, setPassword] = useState('')
  // Which way in is under way, so only that button spins.
  const [busy, setBusy] = useState<'password' | 'code' | 'google' | 'sso' | 'signup' | null>(null)
  const submitting = busy !== null
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

  const workOsSignIn = async (way: 'google' | 'sso' | 'signup') => {
    setBusy(way)
    setError(null)
    try {
      await startWorkOs(
        way === 'signup' ? { username: loginName.trim() } : way === 'google' ? { provider: 'GoogleOAuth' } : {},
      )
    } catch {
      setError(way === 'google' ? 'Google sign-in didn’t open. Try again.' : 'Single sign-on didn’t open. Try again.')
      setBusy(null)
    }
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
    setBusy('password')
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
      setBusy(null)
    }
  }

  const submitCode = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    const trimmed = code.trim()
    if (!challenge || !trimmed) return
    setBusy('code')
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
      setBusy(null)
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
            busy={busy === 'code'}
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

  const google = Boolean(workos.data?.googleEnabled)
  const sso = Boolean(workos.data?.enabled)
  return (
    <AuthLayout
      footer={
        <>
          {sso ? 'Signed up with a password? Link single sign-on in Account settings after signing in. ' : null}
          {resetByEmail.data?.available ? (
            <Link to="/forgot-password">Forgot your password?</Link>
          ) : (
            'Forgot your password? Whoever runs this server can reset it for you.'
          )}
        </>
      }
      intro="Welcome back. Your conversations and agents are waiting."
      title="Sign in"
      titleId="sign-in-heading"
    >
      {new URLSearchParams(location.search).has('workosError') && (
        <Banner tone="danger">
          Single sign-on didn’t finish. If you already have an account, sign in below and link single sign-on in Account settings.
        </Banner>
      )}
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
          label="Username"
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
          busy={busy === 'password'}
          className="aw-auth__submit"
          disabled={!loginName.trim() || password.length < 12 || (submitting && busy !== 'password')}
          size="lg"
          type="submit"
          variant="primary"
        >
          {busy === 'password' ? 'Signing in…' : 'Sign in'}
        </Button>
        {workos.data?.signupEnabled && (
          <Button
            busy={busy === 'signup'}
            disabled={(submitting && busy !== 'signup') || loginName.trim().length < 3}
            onClick={() => void workOsSignIn('signup')}
          >
            Create account with this username
          </Button>
        )}
      </form>
      {/* Below the form, so the field you're typing in doesn't move when these arrive. */}
      {google || sso ? (
        <div className="aw-auth__sso">
          <p className="aw-auth__divider">
            <span>or</span>
          </p>
          {google ? (
            <Button
              busy={busy === 'google'}
              className="aw-auth__submit"
              disabled={submitting && busy !== 'google'}
              onClick={() => void workOsSignIn('google')}
              size="lg"
            >
              Continue with Google
            </Button>
          ) : null}
          {sso ? (
            <Button
              busy={busy === 'sso'}
              className="aw-auth__submit"
              disabled={submitting && busy !== 'sso'}
              onClick={() => void workOsSignIn('sso')}
              size="lg"
            >
              Continue with single sign-on
            </Button>
          ) : null}
        </div>
      ) : null}
    </AuthLayout>
  )
}
