import { FormEvent, useEffect, useRef, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { useNavigate } from 'react-router-dom'

import { isApiError } from '../api/errors'
import {
  installAuthenticatedSession,
  isAuthenticatedSession,
  type SessionResponse,
  useSession,
} from '../auth/session'
import { isTwoFactorChallenge, login, loginTwoFactor } from '../auth/login'
import {
  acceptExistingInvitation,
  acceptNewInvitation,
  resolveInvitation,
  type ResolvedInvitation,
} from './api'
import { AuthLayout } from '../auth/AuthLayout'
import { Banner, Button, TextField } from '../ui'

type JoinMode = 'new' | 'existing'

function invitationTokenFromFragment() {
  const fragment = new URLSearchParams(window.location.hash.slice(1))
  const token = fragment.get('token') ?? ''
  window.history.replaceState(window.history.state, '', '/join')
  return token
}

function joinError(reason: unknown) {
  if (!isApiError(reason)) return 'The invitation could not be accepted. Try again.'
  if (reason.code === 'rate_limited') {
    return 'Too many attempts. Wait a moment, then try again.'
  }
  if (reason.status === 404) {
    return 'This invitation is invalid, expired, or has already been used.'
  }
  if (reason.status === 409) return reason.message
  return 'The invitation could not be accepted. Check the fields and try again.'
}

export function JoinPage() {
  const tokenRef = useRef<string | null>(null)
  if (tokenRef.current === null) tokenRef.current = invitationTokenFromFragment()

  const session = useSession()
  const queryClient = useQueryClient()
  const navigate = useNavigate()
  const errorRef = useRef<HTMLDivElement>(null)
  const [invitation, setInvitation] = useState<ResolvedInvitation | null>(null)
  const [resolving, setResolving] = useState(true)
  const [submitting, setSubmitting] = useState(false)
  const [mode, setMode] = useState<JoinMode>('new')
  const [loginName, setLoginName] = useState('')
  const [displayName, setDisplayName] = useState('')
  const [handle, setHandle] = useState('')
  const [password, setPassword] = useState('')
  const [passwordConfirmation, setPasswordConfirmation] = useState('')
  const [error, setError] = useState<string | null>(null)
  // An existing account with two-factor on signs in with a code before joining.
  const [challenge, setChallenge] = useState<string | null>(null)
  const [code, setCode] = useState('')

  useEffect(() => {
    const token = tokenRef.current ?? ''
    if (!token) {
      setResolving(false)
      setError('This invitation is invalid, expired, or has already been used.')
      return
    }
    let active = true
    void resolveInvitation(token)
      .then((resolved) => {
        if (active) setInvitation(resolved)
      })
      .catch((reason: unknown) => {
        if (active) setError(joinError(reason))
      })
      .finally(() => {
        if (active) setResolving(false)
      })
    return () => {
      active = false
    }
  }, [])

  useEffect(() => {
    if (error) errorRef.current?.focus()
  }, [error])

  const finish = async (nextSession: SessionResponse) => {
    if (!isAuthenticatedSession(nextSession)) {
      throw new Error('Invitation acceptance did not establish a session.')
    }
    installAuthenticatedSession(queryClient, nextSession)
    tokenRef.current = ''
    setPassword('')
    setPasswordConfirmation('')
    navigate('/home', { replace: true })
  }

  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    const token = tokenRef.current ?? ''
    if (!token || !handle.trim()) return
    if (mode === 'new' && password !== passwordConfirmation) {
      setError('The passwords do not match.')
      return
    }
    setSubmitting(true)
    setError(null)
    try {
      if (isAuthenticatedSession(session.data)) {
        await finish(
          await acceptExistingInvitation({ token, handle: handle.trim() }),
        )
      } else if (mode === 'existing') {
        let authenticated
        if (challenge) {
          authenticated = await loginTwoFactor(challenge, code.trim())
        } else {
          authenticated = await login(loginName.trim(), password)
          if (isTwoFactorChallenge(authenticated)) {
            setChallenge(authenticated.challenge)
            setPassword('')
            return
          }
        }
        setChallenge(null)
        setCode('')
        if (!isAuthenticatedSession(authenticated)) {
          throw new Error('Sign-in did not establish a session.')
        }
        installAuthenticatedSession(queryClient, authenticated)
        await finish(
          await acceptExistingInvitation({ token, handle: handle.trim() }),
        )
      } else {
        await finish(
          await acceptNewInvitation({
            token,
            loginName: loginName.trim(),
            displayName: displayName.trim(),
            handle: handle.trim(),
            password,
          }),
        )
      }
    } catch (reason) {
      setPassword('')
      setPasswordConfirmation('')
      if (challenge && isApiError(reason) && reason.status === 404) {
        setChallenge(null)
        setError('That sign-in expired. Enter your password again.')
      } else if (challenge && isApiError(reason) && reason.code === 'invalid_credentials') {
        setCode('')
        setError('That code didn’t work. Check your authenticator app and try again.')
      } else {
        setError(joinError(reason))
      }
    } finally {
      setSubmitting(false)
    }
  }

  const signedIn = isAuthenticatedSession(session.data)
  const invalid = !resolving && !invitation
  const workspace = invitation?.kind === 'workspace'
  const needsCode = !signedIn && mode === 'existing' && challenge !== null

  return (
    <AuthLayout
      footer="Invitation links work once. The secret is removed from the address bar before anything is sent."
      intro={
        resolving ? (
          <span aria-live="polite">Checking your invitation…</span>
        ) : invitation ? (
          workspace ? (
            <>
              Join the <strong>{invitation.organizationName}</strong> workspace as {invitation.role}.
            </>
          ) : (
            <>
              Join <strong>{invitation.projectName}</strong> in {invitation.organizationName} as {invitation.role}.
            </>
          )
        ) : undefined
      }
      kicker="Private invitation"
      title={workspace ? 'Join a workspace' : 'Join a Project'}
      titleId="join-heading"
    >

        {error && (
          <div className="aw-auth__error" ref={errorRef} role="alert" tabIndex={-1}>
            <Banner tone="danger">{error}</Banner>
          </div>
        )}

        {!invalid && invitation && (
          <form className="aw-auth__form" onSubmit={submit}>
            {!signedIn && (
              <fieldset className="aw-auth__mode">
                <legend className="visually-hidden">Account</legend>
                <label>
                  <input
                    checked={mode === 'new'}
                    disabled={submitting}
                    name="join-mode"
                    onChange={() => { setMode('new'); setChallenge(null) }}
                    type="radio"
                  />
                  Create an account
                </label>
                <label>
                  <input
                    checked={mode === 'existing'}
                    disabled={submitting}
                    name="join-mode"
                    onChange={() => setMode('existing')}
                    type="radio"
                  />
                  Use my existing account
                </label>
              </fieldset>
            )}

            {!signedIn && mode === 'new' && (
              <TextField
                autoComplete="name"
                disabled={submitting}
                id="join-display-name"
                label="Display name"
                maxLength={128}
                onChange={(event) => setDisplayName(event.target.value)}
                required
                value={displayName}
              />
            )}
            {!signedIn && (
              <TextField
                autoComplete="username"
                disabled={submitting}
                id="join-login-name"
                label="Login name"
                maxLength={64}
                onChange={(event) => setLoginName(event.target.value)}
                required
                value={loginName}
              />
            )}
            <TextField
              autoComplete="off"
              disabled={submitting}
              id="join-handle"
              label={workspace ? 'Handle' : 'Project handle'}
              maxLength={64}
              onChange={(event) => setHandle(event.target.value)}
              required
              value={handle}
            />
            {!signedIn && !needsCode && (
              <TextField
                autoComplete={mode === 'new' ? 'new-password' : 'current-password'}
                disabled={submitting}
                id="join-password"
                label="Password"
                minLength={12}
                onChange={(event) => setPassword(event.target.value)}
                required
                type="password"
                value={password}
              />
            )}
            {needsCode && (
              <TextField
                autoComplete="one-time-code"
                autoFocus
                disabled={submitting}
                hint="The 6-digit code from your authenticator app, or a recovery code."
                id="join-two-factor-code"
                label="Two-factor code"
                maxLength={32}
                onChange={(event) => setCode(event.target.value)}
                required
                spellCheck={false}
                value={code}
              />
            )}
            {!signedIn && mode === 'new' && (
              <TextField
                autoComplete="new-password"
                disabled={submitting}
                id="join-password-confirmation"
                label="Confirm password"
                minLength={12}
                onChange={(event) => setPasswordConfirmation(event.target.value)}
                required
                type="password"
                value={passwordConfirmation}
              />
            )}
            <Button
              busy={submitting}
              className="aw-auth__submit"
              disabled={
                submitting ||
                !handle.trim() ||
                (needsCode
                  ? !code.trim()
                  : !signedIn && (!loginName.trim() || password.length < 12)) ||
                (!signedIn && mode === 'new' && (!displayName.trim() || password !== passwordConfirmation))
              }
              size="lg"
              type="submit"
              variant="primary"
            >
              {submitting ? 'Joining…' : signedIn ? 'Accept invitation' : mode === 'new' ? 'Create account and join' : 'Sign in and join'}
            </Button>
          </form>
        )}
    </AuthLayout>
  )
}
