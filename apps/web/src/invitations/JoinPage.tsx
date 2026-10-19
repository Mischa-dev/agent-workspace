import { FormEvent, useEffect, useRef, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { useNavigate } from 'react-router-dom'

import { queryKeys } from '../api/queryKeys'
import { isApiError } from '../api/errors'
import {
  clearAuthenticatedQueryCache,
  installAuthenticatedSession,
  isAuthenticatedSession,
  logout,
  type SessionResponse,
  useSession,
} from '../auth/session'
import { isTwoFactorChallenge, login, loginTwoFactor } from '../auth/login'
import { listProjects } from '../projects/api'
import {
  acceptExistingInvitation,
  acceptNewInvitation,
  invitationKind,
  resolveInvitation,
  type ResolvedInvitation,
} from './api'
import { AuthLayout } from '../auth/AuthLayout'
import { Banner, Button, ButtonLink, TextField } from '../ui'

type JoinMode = 'new' | 'existing'

/**
 * The invitation's secret, kept for this tab only until it's accepted, so a
 * reload (or a phone dropping the tab) doesn't lose it once the address bar
 * has been cleaned.
 */
const TOKEN_KEY = 'join:invitation'

function rememberToken(token: string | null) {
  try {
    if (token) window.sessionStorage.setItem(TOKEN_KEY, token)
    else window.sessionStorage.removeItem(TOKEN_KEY)
  } catch {
    // Without storage, a reload just asks for the link again.
  }
}

function invitationToken() {
  const fragment = new URLSearchParams(window.location.hash.slice(1))
  const fromLink = fragment.get('token') ?? ''
  window.history.replaceState(window.history.state, '', '/join')
  if (fromLink) {
    rememberToken(fromLink)
    return fromLink
  }
  try {
    return window.sessionStorage.getItem(TOKEN_KEY) ?? ''
  } catch {
    return ''
  }
}

const INVALID = 'This invitation is invalid, expired, or has already been used.'

function joinError(reason: unknown, mode: JoinMode | 'signed-in') {
  if (!isApiError(reason)) return 'The invitation wasn’t accepted. Try again.'
  if (reason.code === 'rate_limited') return 'Too many attempts. Wait a moment, then try again.'
  if (reason.status === 404) return INVALID
  if (reason.status === 409) {
    return mode === 'new'
      ? 'That username or handle is already taken. Try another.'
      : 'That handle is already taken here, or this account was invited before. Try another handle.'
  }
  if (reason.status === 400 || reason.status === 422) {
    return reason.code === 'validation_failed'
      ? 'Check your username and handle: letters, numbers, - and _ only.'
      : reason.message
  }
  return 'The invitation wasn’t accepted. Try again.'
}

/** A handle to start from: "new.human" becomes "new_human". */
function handleFrom(name: string) {
  return name
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9_-]+/g, '_')
    .replace(/^[^a-z0-9]+/, '')
    .slice(0, 32)
}

const HANDLE = /^[a-z0-9][a-z0-9_-]{1,31}$/

function roleText(role: ResolvedInvitation['role']) {
  return role === 'admin' ? 'an admin' : role === 'viewer' ? 'a viewer' : 'a member'
}

export function JoinPage() {
  // Read once: the token leaves the address bar, and what it invites to titles the page until the server answers.
  const [initial] = useState(() => {
    const token = invitationToken()
    return { token, kind: token ? invitationKind(token) : null }
  })
  const tokenRef = useRef<string | null>(initial.token)

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
  const [typedHandle, setTypedHandle] = useState<string | null>(null)
  const [password, setPassword] = useState('')
  const [passwordConfirmation, setPasswordConfirmation] = useState('')
  const [error, setError] = useState<string | null>(null)
  // An existing account with two-factor on signs in with a code before joining.
  const [challenge, setChallenge] = useState<string | null>(null)
  const [code, setCode] = useState('')

  const signedIn = isAuthenticatedSession(session.data)
  const viewer = isAuthenticatedSession(session.data) ? session.data.user : null
  // Until someone types one, the handle follows the username.
  const handle = typedHandle ?? handleFrom(viewer?.loginName ?? loginName)

  useEffect(() => {
    const token = tokenRef.current ?? ''
    if (!token) {
      setResolving(false)
      setError(INVALID)
      return
    }
    let active = true
    void resolveInvitation(token)
      .then((resolved) => {
        if (active) setInvitation(resolved)
      })
      .catch((reason: unknown) => {
        if (!active) return
        if (isApiError(reason) && reason.status === 404) rememberToken(null)
        setError(joinError(reason, 'new'))
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

  /** A project invitation lands in that project when it can be told apart; otherwise Home. */
  const landing = async () => {
    if (invitation?.kind !== 'project' || !invitation.projectName) return '/home'
    try {
      const { projects } = await queryClient.fetchQuery({ queryKey: queryKeys.projects, queryFn: () => listProjects() })
      const matches = projects.filter((project) => project.name === invitation.projectName && !project.archivedAt)
      return matches.length === 1 ? `/projects/${encodeURIComponent(matches[0].id)}` : '/home'
    } catch {
      return '/home'
    }
  }

  const finish = async (nextSession: SessionResponse) => {
    if (!isAuthenticatedSession(nextSession)) {
      throw new Error('Invitation acceptance did not establish a session.')
    }
    installAuthenticatedSession(queryClient, nextSession)
    tokenRef.current = ''
    rememberToken(null)
    setPassword('')
    setPasswordConfirmation('')
    navigate(await landing(), { replace: true })
  }

  const signOut = async () => {
    try {
      await logout()
      clearAuthenticatedQueryCache(queryClient)
    } catch {
      setError('You’re still signed in. Try again.')
    }
  }

  /** What's missing, named, so the button never just sits there disabled. */
  const missing = () => {
    if (!signedIn && mode === 'new' && !displayName.trim()) return 'Enter your display name.'
    if (!signedIn && !challenge && !loginName.trim()) return 'Enter your username.'
    if (!HANDLE.test(handle.trim().toLowerCase())) return 'Handles use 2–32 letters, numbers, - or _, starting with a letter or number.'
    if (!signedIn && challenge && !code.trim()) return 'Enter the code from your authenticator app.'
    if (!signedIn && !challenge && password.length < 12) return 'Passwords are at least 12 characters.'
    if (!signedIn && mode === 'new' && password !== passwordConfirmation) return 'The passwords don’t match.'
    return null
  }

  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    const token = tokenRef.current ?? ''
    if (!token) return
    const problem = missing()
    if (problem) {
      setError(problem)
      return
    }
    setSubmitting(true)
    setError(null)
    try {
      if (isAuthenticatedSession(session.data)) {
        await finish(await acceptExistingInvitation({ token, handle: handle.trim() }))
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
        await finish(await acceptExistingInvitation({ token, handle: handle.trim() }))
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
        setError(joinError(reason, signedIn ? 'signed-in' : mode))
      }
    } finally {
      setSubmitting(false)
    }
  }

  const invalid = !resolving && !invitation
  // Known from the link itself, so the title doesn't change once the invitation loads.
  const kind = invitation?.kind ?? initial.kind
  const workspace = kind === 'workspace'
  const needsCode = !signedIn && mode === 'existing' && challenge !== null
  const mismatch =
    mode === 'new' && passwordConfirmation.length > 0 && passwordConfirmation.length >= password.length && passwordConfirmation !== password

  return (
    <AuthLayout
      footer="Invitation links work once. The secret is removed from the address bar before anything is sent."
      intro={
        resolving ? (
          <span aria-live="polite">Checking your invitation…</span>
        ) : invitation ? (
          workspace ? (
            <>
              Join the <strong>{invitation.organizationName}</strong> workspace as {roleText(invitation.role)}.
            </>
          ) : (
            <>
              Join <strong>{invitation.projectName}</strong> in {invitation.organizationName} as {roleText(invitation.role)}.
            </>
          )
        ) : undefined
      }
      kicker="Private invitation"
      title={kind === 'workspace' ? 'Join a workspace' : kind === 'project' ? 'Join a project' : 'Join'}
      titleId="join-heading"
    >
      {error && (
        <div className="aw-auth__error" ref={errorRef} role="alert" tabIndex={-1}>
          <Banner tone="danger">{error}</Banner>
        </div>
      )}

      {invalid ? (
        <ButtonLink className="aw-auth__submit" to={signedIn ? '/home' : '/sign-in'} variant="primary">
          {signedIn ? 'Go to Home' : 'Go to sign in'}
        </ButtonLink>
      ) : null}

      {!invalid && invitation && (
        <form className="aw-auth__form" noValidate onSubmit={submit}>
          {viewer ? (
            <p className="aw-auth__whoami">
              Joining as <strong>{viewer.displayName}</strong>.{' '}
              <button className="aw-auth__linkbutton" disabled={submitting} onClick={() => void signOut()} type="button">
                Not you? Sign out
              </button>
            </p>
          ) : (
            <fieldset className="aw-auth__mode">
              <legend className="visually-hidden">Account</legend>
              <label>
                <input
                  checked={mode === 'new'}
                  disabled={submitting}
                  name="join-mode"
                  onChange={() => {
                    setMode('new')
                    setChallenge(null)
                  }}
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
              disabled={submitting || needsCode}
              id="join-login-name"
              label="Username"
              maxLength={64}
              onChange={(event) => setLoginName(event.target.value)}
              required
              value={loginName}
            />
          )}
          <TextField
            autoComplete="off"
            disabled={submitting}
            hint={workspace ? 'How people mention you in this workspace.' : 'How people mention you in this project.'}
            id="join-handle"
            label="Handle"
            maxLength={32}
            onChange={(event) => setTypedHandle(event.target.value.replace(/^@/, ''))}
            prefix="@"
            required
            spellCheck={false}
            value={handle}
          />
          {!signedIn && !needsCode && (
            <TextField
              autoComplete={mode === 'new' ? 'new-password' : 'current-password'}
              disabled={submitting}
              hint={mode === 'new' ? 'At least 12 characters.' : undefined}
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
              error={mismatch ? 'The passwords don’t match.' : undefined}
              id="join-password-confirmation"
              label="Confirm password"
              minLength={12}
              onChange={(event) => setPasswordConfirmation(event.target.value)}
              required
              type="password"
              value={passwordConfirmation}
            />
          )}
          <Button busy={submitting} className="aw-auth__submit" size="lg" type="submit" variant="primary">
            {submitting ? 'Joining…' : signedIn ? 'Accept invitation' : mode === 'new' ? 'Create account and join' : 'Sign in and join'}
          </Button>
        </form>
      )}
    </AuthLayout>
  )
}
