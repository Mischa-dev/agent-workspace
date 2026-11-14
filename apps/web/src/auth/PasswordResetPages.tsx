// Forgot password, choosing a new one from an emailed link, and confirming
// an email address. Links carry their token after `#`, so it never reaches
// the server's logs or another site's Referer; the page sends it in a POST.
import { useEffect, useRef, useState, type FormEvent } from 'react'
import { useQuery } from '@tanstack/react-query'
import { Link, useLocation } from 'react-router-dom'

import { requestJson } from '../api/client'
import { isApiError } from '../api/errors'
import { Banner, Button, ButtonLink, Checkbox, TextField, describeError, describeLoadError } from '../ui'
import { AuthLayout } from './AuthLayout'
import './auth.css'

type ResetInspection = { loginName: string; twoFactorRequired: boolean }

const passwordResetAvailable = () => requestJson<{ available: boolean }>('/api/v1/auth/password-reset')

const requestReset = (login: string) => requestJson<{ accepted: boolean }>('/api/v1/auth/password-reset', { method: 'POST', json: { login } })

const inspectReset = (token: string) => requestJson<ResetInspection>('/api/v1/auth/password-reset/inspect', { method: 'POST', json: { token } })

const completeReset = (token: string, newPassword: string, code: string | null, disconnectApps: boolean) =>
  requestJson<void>('/api/v1/auth/password-reset/complete', {
    method: 'POST',
    json: { token, newPassword, ...(code ? { code } : {}), ...(disconnectApps ? { disconnectApps } : {}) },
  })

const verifyEmail = (token: string) => requestJson<{ address: string }>('/api/v1/auth/verify-email', { method: 'POST', json: { token } })

/** The token after `#token=`, then the address bar is cleaned so it isn't left in history. */
function useHashToken() {
  const location = useLocation()
  const [token] = useState(() => new URLSearchParams(location.hash.replace(/^#/, '')).get('token')?.trim() ?? '')
  useEffect(() => {
    if (token && window.location.hash) window.history.replaceState(window.history.state, '', window.location.pathname)
  }, [token])
  return token
}

function rateLimited(reason: unknown) {
  return isApiError(reason) && reason.status === 429
}

function ErrorBanner({ message }: { message: string | null }) {
  const ref = useRef<HTMLDivElement>(null)
  useEffect(() => {
    if (message) ref.current?.focus()
  }, [message])
  if (!message) return null
  return (
    <div className="aw-auth__error" ref={ref} role="alert" tabIndex={-1}>
      <Banner tone="danger">{message}</Banner>
    </div>
  )
}

export function ForgotPasswordPage() {
  const available = useQuery({ queryKey: ['password-reset-available'], queryFn: passwordResetAvailable })
  const [login, setLogin] = useState('')
  const [sent, setSent] = useState(false)
  const [submitting, setSubmitting] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const submit = async (event: FormEvent) => {
    event.preventDefault()
    if (!login.trim()) return
    setSubmitting(true)
    setError(null)
    try {
      await requestReset(login.trim())
      setSent(true)
    } catch (reason) {
      setError(rateLimited(reason) ? 'Too many requests. Wait a while, then try again.' : `The link wasn’t sent. ${describeError(reason).message}`)
    } finally {
      setSubmitting(false)
    }
  }

  if (available.data && !available.data.available) {
    return (
      <AuthLayout footer={<Link to="/sign-in">Back to sign in</Link>} title="Forgot your password?" titleId="forgot-heading">
        <p>This server can’t send email yet, so whoever runs it resets passwords. Ask them to run the password reset command for your username.</p>
      </AuthLayout>
    )
  }
  if (sent) {
    return (
      <AuthLayout footer={<Link to="/sign-in">Back to sign in</Link>} title="Check your email" titleId="forgot-heading">
        <p>
          If that account has a confirmed email address, a link to choose a new password is on its way. It works once, for 30
          minutes.
        </p>
        <p className="aw-quiet">Nothing arriving? Check spam, or ask whoever runs this server.</p>
      </AuthLayout>
    )
  }
  return (
    <AuthLayout
      footer={<Link to="/sign-in">Back to sign in</Link>}
      intro="Enter your username or your confirmed email address. A link to choose a new password goes to that account’s email."
      title="Forgot your password?"
      titleId="forgot-heading"
    >
      <ErrorBanner message={error} />
      <form className="aw-auth__form" onSubmit={submit}>
        <TextField autoComplete="username" autoFocus disabled={submitting} id="reset-login" label="Username or email" maxLength={254} onChange={(event) => setLogin(event.target.value)} required value={login} />
        <Button busy={submitting} className="aw-auth__submit" disabled={!login.trim()} size="lg" type="submit" variant="primary">
          {submitting ? 'Sending…' : 'Email me a link'}
        </Button>
      </form>
    </AuthLayout>
  )
}

export function ResetPasswordPage() {
  const token = useHashToken()
  const inspection = useQuery({
    queryKey: ['password-reset', token],
    queryFn: () => inspectReset(token),
    enabled: Boolean(token),
    retry: false,
    staleTime: Infinity,
  })
  const [password, setPassword] = useState('')
  const [confirm, setConfirm] = useState('')
  const [code, setCode] = useState('')
  const [disconnectApps, setDisconnectApps] = useState(false)
  const [submitting, setSubmitting] = useState(false)
  const [done, setDone] = useState(false)
  const [expired, setExpired] = useState(false)
  const [error, setError] = useState<string | null>(null)

  // Only the server saying no means the link is bad; a dropped connection or a server fault can be tried again.
  const linkRefused =
    isApiError(inspection.error) && inspection.error.status >= 400 && inspection.error.status < 500 && inspection.error.status !== 429
  if (!token || expired || linkRefused) {
    return (
      <AuthLayout footer={<Link to="/sign-in">Back to sign in</Link>} title="This link doesn’t work" titleId="reset-heading">
        <p>Reset links work once, for 30 minutes, and stop working when the password changes some other way.</p>
        <ButtonLink className="aw-auth__submit" to="/forgot-password" variant="primary">
          Get a new link
        </ButtonLink>
      </AuthLayout>
    )
  }
  if (inspection.isError) {
    return (
      <AuthLayout footer={<Link to="/sign-in">Back to sign in</Link>} title="Your link wasn’t checked" titleId="reset-heading">
        <p>{describeLoadError(inspection.error)}</p>
        <Button className="aw-auth__submit" icon="retry" onClick={() => void inspection.refetch()} variant="primary">
          Try again
        </Button>
      </AuthLayout>
    )
  }
  if (done) {
    return (
      <AuthLayout title="Password changed" titleId="reset-heading">
        <p>You’re signed out everywhere. Sign in with your new password.</p>
        <ButtonLink className="aw-auth__submit" to="/sign-in" variant="primary">
          Sign in
        </ButtonLink>
      </AuthLayout>
    )
  }
  if (inspection.isPending) {
    return (
      <AuthLayout title="Choose a new password" titleId="reset-heading">
        <p className="aw-quiet" role="status">Checking your link…</p>
      </AuthLayout>
    )
  }

  const twoFactor = inspection.data.twoFactorRequired
  // Only once the second one is as long as the first, so it doesn't complain while you're still typing.
  const mismatch = confirm.length > 0 && confirm.length >= password.length && confirm !== password
  const ready = password.length >= 12 && password === confirm && (!twoFactor || code.trim().length > 0)
  const submit = async (event: FormEvent) => {
    event.preventDefault()
    if (!ready) return
    setSubmitting(true)
    setError(null)
    try {
      await completeReset(token, password, twoFactor ? code.trim() : null, disconnectApps)
      setPassword('')
      setConfirm('')
      setDone(true)
    } catch (reason) {
      setCode('')
      if (isApiError(reason) && reason.status === 404) setExpired(true)
      else if (isApiError(reason) && reason.code === 'invalid_credentials') setError('That code didn’t work. Check your authenticator app and try again.')
      else if (rateLimited(reason)) setError('Too many attempts. Wait a moment, then try again.')
      else setError(`The password wasn’t changed. ${describeError(reason).message}`)
    } finally {
      setSubmitting(false)
    }
  }
  return (
    <AuthLayout intro={`For ${inspection.data.loginName}. Changing it signs you out everywhere.`} title="Choose a new password" titleId="reset-heading">
      <ErrorBanner message={error} />
      <form className="aw-auth__form" onSubmit={submit}>
        <input autoComplete="username" hidden readOnly value={inspection.data.loginName} />
        <TextField autoComplete="new-password" autoFocus disabled={submitting} hint="At least 12 characters." id="new-password" label="New password" minLength={12} onChange={(event) => setPassword(event.target.value)} required type="password" value={password} />
        <TextField autoComplete="new-password" disabled={submitting} error={mismatch ? 'The passwords don’t match.' : undefined} id="confirm-password" label="New password again" minLength={12} onChange={(event) => setConfirm(event.target.value)} required type="password" value={confirm} />
        {twoFactor && (
          <TextField autoComplete="one-time-code" disabled={submitting} hint="From your authenticator app, or a recovery code." id="reset-code" label="Two-factor code" maxLength={32} mono onChange={(event) => setCode(event.target.value)} required spellCheck={false} value={code} />
        )}
        <Checkbox
          checked={disconnectApps}
          description="If someone else may have got in. You’ll need to connect each app again."
          disabled={submitting}
          label="Also disconnect my connected apps"
          onChange={setDisconnectApps}
        />
        <Button busy={submitting} className="aw-auth__submit" disabled={!ready} size="lg" type="submit" variant="primary">
          {submitting ? 'Saving…' : 'Change password'}
        </Button>
      </form>
    </AuthLayout>
  )
}

export function VerifyEmailPage() {
  const token = useHashToken()
  const result = useQuery({
    queryKey: ['verify-email', token],
    queryFn: () => verifyEmail(token),
    enabled: Boolean(token),
    retry: false,
    staleTime: Infinity,
  })
  if (token && result.isPending) {
    return (
      <AuthLayout title="Confirming your email" titleId="verify-heading">
        <p className="aw-quiet" role="status">One moment…</p>
      </AuthLayout>
    )
  }
  if (result.data) {
    return (
      <AuthLayout title="Email confirmed" titleId="verify-heading">
        <p>Password resets and the notifications you choose to get by email now go to {result.data.address}.</p>
        <ButtonLink className="aw-auth__submit" to="/settings/account/notifications" variant="primary">
          Choose email notifications
        </ButtonLink>
      </AuthLayout>
    )
  }
  const taken = isApiError(result.error) && result.error.status === 409
  return (
    <AuthLayout title="This link doesn’t work" titleId="verify-heading">
      <p>
        {taken
          ? 'Another account already confirmed this address.'
          : 'Confirmation links work for 24 hours, and only the newest one works. Send a new one from your profile.'}
      </p>
      <ButtonLink className="aw-auth__submit" to="/settings/account/profile" variant="primary">
        Open your profile
      </ButtonLink>
    </AuthLayout>
  )
}
