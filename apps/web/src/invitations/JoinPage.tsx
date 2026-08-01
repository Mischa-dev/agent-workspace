import { FormEvent, useEffect, useRef, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { useNavigate } from 'react-router-dom'

import { isApiError } from '../api/errors'
import {
  installAuthenticatedSession,
  isAuthenticatedSession,
  login,
  type SessionResponse,
  useSession,
} from '../auth/session'
import {
  acceptExistingInvitation,
  acceptNewInvitation,
  resolveInvitation,
  type ResolvedInvitation,
} from './api'

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
        const authenticated = await login(loginName.trim(), password)
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
      setError(joinError(reason))
    } finally {
      setSubmitting(false)
    }
  }

  const signedIn = isAuthenticatedSession(session.data)
  const invalid = !resolving && !invitation

  return (
    <main className="sign-in-page join-page">
      <section aria-labelledby="join-heading" className="sign-in-panel join-panel">
        <div className="brand-mark" aria-hidden="true">A</div>
        <div>
          <span className="section-label">Private invitation</span>
          <h1 id="join-heading">Join a Project</h1>
          {resolving && <p aria-live="polite">Checking invitation…</p>}
          {invitation && (
            <p className="sign-in-intro">
              Join <strong>{invitation.projectName}</strong> in{' '}
              {invitation.organizationName} as {invitation.role}.
            </p>
          )}
        </div>

        {error && (
          <div className="form-error-summary" ref={errorRef} role="alert" tabIndex={-1}>
            {error}
          </div>
        )}

        {!invalid && invitation && (
          <form onSubmit={submit}>
            {!signedIn && (
              <fieldset className="join-mode">
                <legend>Identity</legend>
                <label>
                  <input
                    checked={mode === 'new'}
                    disabled={submitting}
                    name="join-mode"
                    onChange={() => setMode('new')}
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
              <div className="field">
                <label htmlFor="join-display-name">Display name</label>
                <input
                  autoComplete="name"
                  disabled={submitting}
                  id="join-display-name"
                  maxLength={128}
                  onChange={(event) => setDisplayName(event.target.value)}
                  required
                  value={displayName}
                />
              </div>
            )}
            {!signedIn && (
              <div className="field">
                <label htmlFor="join-login-name">Login name</label>
                <input
                  autoComplete="username"
                  disabled={submitting}
                  id="join-login-name"
                  maxLength={64}
                  onChange={(event) => setLoginName(event.target.value)}
                  required
                  value={loginName}
                />
              </div>
            )}
            <div className="field">
              <label htmlFor="join-handle">Project handle</label>
              <input
                autoComplete="off"
                disabled={submitting}
                id="join-handle"
                maxLength={64}
                onChange={(event) => setHandle(event.target.value)}
                required
                value={handle}
              />
            </div>
            {!signedIn && (
              <div className="field">
                <label htmlFor="join-password">Password</label>
                <input
                  autoComplete={mode === 'new' ? 'new-password' : 'current-password'}
                  disabled={submitting}
                  id="join-password"
                  minLength={12}
                  onChange={(event) => setPassword(event.target.value)}
                  required
                  type="password"
                  value={password}
                />
              </div>
            )}
            {!signedIn && mode === 'new' && (
              <div className="field">
                <label htmlFor="join-password-confirmation">Confirm password</label>
                <input
                  autoComplete="new-password"
                  disabled={submitting}
                  id="join-password-confirmation"
                  minLength={12}
                  onChange={(event) => setPasswordConfirmation(event.target.value)}
                  required
                  type="password"
                  value={passwordConfirmation}
                />
              </div>
            )}
            <button
              className="primary-button sign-in-submit"
              disabled={
                submitting ||
                !handle.trim() ||
                (!signedIn && (!loginName.trim() || password.length < 12)) ||
                (!signedIn && mode === 'new' && (!displayName.trim() || password !== passwordConfirmation))
              }
              type="submit"
            >
              {submitting ? 'Joining…' : signedIn ? 'Accept invitation' : mode === 'new' ? 'Create account and join' : 'Sign in and join'}
            </button>
          </form>
        )}

        <p className="operator-note">
          Invitation links are single-use. Agent Workspace removes the secret from the address bar before contacting the server.
        </p>
      </section>
    </main>
  )
}
