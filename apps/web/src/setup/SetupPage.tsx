import { useEffect, useRef, useState, type FormEvent } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Navigate } from 'react-router-dom'

import { isApiError } from '../api/errors'
import { AuthLayout } from '../auth/AuthLayout'
import { installAuthenticatedSession, isAuthenticatedSession } from '../auth/session'
import { formatBytes } from '../files/model'
import { Banner, Button, SelectField, TextField } from '../ui'
import { setUp, setupKeys, setupStatus, uploadArchive, type SetupShape } from './api'
import '../auth/auth.css'

// First run in the browser (H1.14): the setup code from the server's data
// folder, the first account, then a new workspace or an archive from
// another server. The server checks everything; this page only asks.

type Way = 'new' | 'import'

const describe = (reason: unknown, fallback: string) => {
  if (isApiError(reason) && reason.code === 'rate_limited') return 'Too many attempts. Wait a moment, then try again.'
  return isApiError(reason) ? reason.message : fallback
}

/** Opens the app afresh, so the new session and workspace load whole. */
const openApp = () => window.location.assign('/')

export function SetupPage() {
  const status = useQuery({ queryKey: setupKeys.status, queryFn: setupStatus })
  if (status.isPending) return null
  if (status.isError) {
    return (
      <AuthLayout title="Set up this server" titleId="setup-heading">
        <Banner tone="danger">{describe(status.error, 'The server didn’t answer. Reload to try again.')}</Banner>
      </AuthLayout>
    )
  }
  if (status.data.awaitingImport) return <ImportStep maxBytes={status.data.maxArchiveBytes} />
  if (!status.data.needed) return <Navigate replace to="/sign-in" />
  return <FirstAccount shapes={status.data.shapes} />
}

function FirstAccount({ shapes }: { shapes: SetupShape[] }) {
  const queryClient = useQueryClient()
  const errorRef = useRef<HTMLDivElement>(null)
  const [code, setCode] = useState('')
  const [displayName, setDisplayName] = useState('')
  const [loginName, setLoginName] = useState('')
  const [password, setPassword] = useState('')
  const [way, setWay] = useState<Way>('new')
  const [workspaceName, setWorkspaceName] = useState('')
  const [shape, setShape] = useState(shapes[0]?.id ?? '')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    if (error) errorRef.current?.focus()
  }, [error])

  const ready =
    code.trim() && displayName.trim() && loginName.trim() && password.length >= 12 && (way === 'import' || workspaceName.trim())
  const chosen = shapes.find((candidate) => candidate.id === shape)

  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!ready) return
    setBusy(true)
    setError(null)
    try {
      const session = await setUp({
        code: code.trim(),
        person: { displayName: displayName.trim(), loginName: loginName.trim(), password },
        workspace: way === 'new' ? { name: workspaceName.trim(), shape: shape || undefined } : undefined,
      })
      setPassword('')
      if (isAuthenticatedSession(session)) installAuthenticatedSession(queryClient, session)
      if (way === 'new') openApp()
      else await queryClient.invalidateQueries({ queryKey: setupKeys.status })
    } catch (reason) {
      setError(describe(reason, 'Setup didn’t go through. Try again.'))
    } finally {
      setBusy(false)
    }
  }

  return (
    <AuthLayout
      intro="Nobody has an account on this server yet. The setup code is in the server’s data folder, in the file named setup-code."
      kicker="First run"
      title="Set up this server"
      titleId="setup-heading"
    >
      {error && (
        <div className="aw-auth__error" ref={errorRef} role="alert" tabIndex={-1}>
          <Banner tone="danger">{error}</Banner>
        </div>
      )}
      <form className="aw-auth__form" onSubmit={submit}>
        <TextField
          autoComplete="off"
          autoFocus
          disabled={busy}
          id="setup-code"
          label="Setup code"
          maxLength={128}
          mono
          onChange={(event) => setCode(event.target.value)}
          required
          spellCheck={false}
          value={code}
        />
        <TextField
          autoComplete="name"
          disabled={busy}
          id="setup-display-name"
          label="Your name"
          maxLength={120}
          onChange={(event) => setDisplayName(event.target.value)}
          required
          value={displayName}
        />
        <TextField
          autoComplete="username"
          disabled={busy}
          hint="What you sign in with."
          id="setup-login-name"
          label="Sign-in name"
          maxLength={64}
          onChange={(event) => setLoginName(event.target.value)}
          required
          spellCheck={false}
          value={loginName}
        />
        <TextField
          autoComplete="new-password"
          disabled={busy}
          hint="At least 12 characters."
          id="setup-password"
          label="Password"
          minLength={12}
          onChange={(event) => setPassword(event.target.value)}
          required
          type="password"
          value={password}
        />
        <fieldset className="aw-auth__mode">
          <legend className="sr-only">What to start with</legend>
          <label>
            <input checked={way === 'new'} disabled={busy} name="setup-way" onChange={() => setWay('new')} type="radio" />
            A new workspace
          </label>
          <label>
            <input checked={way === 'import'} disabled={busy} name="setup-way" onChange={() => setWay('import')} type="radio" />
            Bring one from another server
          </label>
        </fieldset>
        {way === 'new' ? (
          <>
            <TextField
              disabled={busy}
              id="setup-workspace-name"
              label="Workspace name"
              maxLength={120}
              onChange={(event) => setWorkspaceName(event.target.value)}
              required
              value={workspaceName}
            />
            {shapes.length > 0 && (
              <SelectField
                disabled={busy}
                hint={chosen?.description}
                id="setup-shape"
                label="What it’s for"
                onChange={(event) => setShape(event.target.value)}
                value={shape}
              >
                {shapes.map((candidate) => (
                  <option key={candidate.id} value={candidate.id}>
                    {candidate.name}
                  </option>
                ))}
              </SelectField>
            )}
          </>
        ) : (
          <p className="aw-auth__intro">
            Make your account first; next you send the archive exported from the other server.
          </p>
        )}
        <Button busy={busy} className="aw-auth__submit" disabled={!ready} size="lg" type="submit" variant="primary">
          {way === 'new' ? 'Make the account and workspace' : 'Make the account'}
        </Button>
      </form>
    </AuthLayout>
  )
}

function ImportStep({ maxBytes }: { maxBytes: number }) {
  const errorRef = useRef<HTMLDivElement>(null)
  const [archive, setArchive] = useState<File | null>(null)
  const [busy, setBusy] = useState(false)
  const [started, setStarted] = useState(false)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    if (error) errorRef.current?.focus()
  }, [error])

  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!archive) return
    if (archive.size > maxBytes) {
      setError(`That archive is larger than this server takes (${formatBytes(maxBytes)}).`)
      return
    }
    setBusy(true)
    setError(null)
    try {
      await uploadArchive(archive)
      setStarted(true)
    } catch (reason) {
      setError(describe(reason, 'The archive didn’t upload. Try again.'))
    } finally {
      setBusy(false)
    }
  }

  if (started) {
    return (
      <AuthLayout
        intro="The server is bringing in the workspace now. It opens as it was once the import finishes; Settings › Export shows how far it got."
        title="Import started"
        titleId="setup-heading"
      >
        <Button className="aw-auth__submit" onClick={openApp} size="lg" variant="primary">
          Open the workspace
        </Button>
      </AuthLayout>
    )
  }

  return (
    <AuthLayout
      intro="Choose the archive exported from the other server (Settings › Export there). It is checked whole before anything is written."
      kicker="First run"
      title="Bring in a workspace"
      titleId="setup-heading"
    >
      {error && (
        <div className="aw-auth__error" ref={errorRef} role="alert" tabIndex={-1}>
          <Banner tone="danger">{error}</Banner>
        </div>
      )}
      <form className="aw-auth__form" onSubmit={submit}>
        <div className="field">
          <label htmlFor="setup-archive">Archive</label>
          <input
            accept=".awx,.zip,application/zip"
            disabled={busy}
            id="setup-archive"
            onChange={(event) => setArchive(event.target.files?.[0] ?? null)}
            required
            type="file"
          />
        </div>
        <Button busy={busy} className="aw-auth__submit" disabled={!archive} size="lg" type="submit" variant="primary">
          {busy ? 'Uploading…' : 'Import'}
        </Button>
      </form>
    </AuthLayout>
  )
}
