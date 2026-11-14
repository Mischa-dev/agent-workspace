import { useId, useState, type FormEvent } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { isApiError } from '../../api/errors'
import { describeError, toastError } from '../../app/mutations'
import {
  Banner,
  Button,
  Dialog,
  ErrorState,
  Icon,
  ListSkeleton,
  PageHeader,
  Row,
  RowList,
  Section,
  SettingsSkeleton,
  StatusChip,
  TextField,
  countLabel,
  describeLoadError,
  formatFullDate,
  formatRelative,
  formatShortDate,
  showToast,
} from '../../ui'
import { actionPhrase, actionSentence } from '../auditActions'
import type { SettingsPageProps } from '../types'
import {
  accountKeys,
  changePassword,
  confirmTwoFactor,
  describeDevice,
  disableTwoFactor,
  getSecurity,
  listSessions,
  regenerateRecoveryCodes,
  revokeOtherSessions,
  revokeSession,
  startTwoFactorSetup,
  type AccountSecurity,
  type AccountSession,
  type TwoFactorSetup,
} from './api'
import '../../styles/pages.css'

const SIGN_IN_METHODS: Record<string, string> = {
  password: 'Password',
  workos: 'Single sign-on',
  invitation: 'Invitation',
}

function codeError(reason: unknown) {
  if (isApiError(reason) && (reason.code === 'invalid_credentials' || reason.status === 401)) {
    return 'That didn’t match. Check the code or password and try again.'
  }
  return describeError(reason).message
}

/** Account › Security: password, sessions, two-factor and recent sign-ins. */
export default function SecurityPage({ session }: SettingsPageProps) {
  const security = useQuery({ queryKey: accountKeys.security, queryFn: getSecurity })
  return (
    <>
      <PageHeader icon="shield-check" title="Security">
        Your password, where you’re signed in, and two-factor authentication.
      </PageHeader>
      {security.isPending ? (
        <SettingsSkeleton label="Loading security settings" sections={3} />
      ) : security.isError ? (
        <ErrorState error={security.error} onRetry={() => void security.refetch()} title="Security settings didn’t load" />
      ) : (
        <>
          <PasswordSection hasPassword={security.data.hasPassword} />
          <TwoFactorSection loginName={session.user.loginName} security={security.data} />
          <SessionsSection />
          <ActivitySection events={security.data.recentEvents} />
        </>
      )}
    </>
  )
}

function PasswordSection({ hasPassword }: { hasPassword: boolean }) {
  const queryClient = useQueryClient()
  const [current, setCurrent] = useState('')
  const [next, setNext] = useState('')
  const [confirmation, setConfirmation] = useState('')
  const [error, setError] = useState<string | null>(null)
  const change = useMutation({
    mutationFn: () => changePassword(current, next),
    onSuccess: () => {
      setCurrent('')
      setNext('')
      setConfirmation('')
      setError(null)
      showToast({ tone: 'success', title: 'Password changed', description: 'You were signed out everywhere else.' })
      void queryClient.invalidateQueries({ queryKey: accountKeys.sessions })
      void queryClient.invalidateQueries({ queryKey: accountKeys.security })
    },
    onError: (reason) => {
      setCurrent('')
      setError(
        isApiError(reason) && reason.code === 'invalid_credentials'
          ? 'Your current password isn’t right.'
          : describeError(reason).message,
      )
    },
  })
  const mismatch = confirmation.length > 0 && next !== confirmation
  const tooShort = next.length > 0 && next.length < 12
  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!current || next.length < 12 || next !== confirmation) return
    change.mutate()
  }

  return (
    <Section
      description={hasPassword ? 'Changing it signs you out on every other device.' : undefined}
      id="password-heading"
      title="Password"
    >
      {hasPassword ? (
        <form className="aw-card aw-formcard aw-form aw-security__password" onSubmit={submit}>
          <TextField
            autoComplete="current-password"
            id="current-password"
            label="Current password"
            onChange={(event) => setCurrent(event.target.value)}
            required
            type="password"
            value={current}
          />
          <div className="aw-form__grid">
            <TextField
              autoComplete="new-password"
              error={tooShort ? 'Use at least 12 characters.' : null}
              id="new-password"
              label="New password"
              maxLength={1024}
              onChange={(event) => setNext(event.target.value)}
              required
              type="password"
              value={next}
            />
            <TextField
              autoComplete="new-password"
              error={mismatch ? 'The passwords don’t match.' : null}
              id="new-password-confirmation"
              label="Confirm new password"
              maxLength={1024}
              onChange={(event) => setConfirmation(event.target.value)}
              required
              type="password"
              value={confirmation}
            />
          </div>
          {error && <Banner tone="danger">{error}</Banner>}
          <div className="aw-form__actions">
            <Button
              busy={change.isPending}
              disabled={!current || next.length < 12 || next !== confirmation}
              type="submit"
              variant="primary"
            >
              Change password
            </Button>
          </div>
        </form>
      ) : (
        <p className="aw-quiet">You sign in with single sign-on, so there’s no password to change here.</p>
      )}
    </Section>
  )
}

type SetupStep = { step: 'password' } | { step: 'code'; setup: TwoFactorSetup } | { step: 'codes'; codes: string[] }

function TwoFactorSection({ loginName, security }: { loginName: string; security: AccountSecurity }) {
  const queryClient = useQueryClient()
  const twoFactor = security.twoFactor
  const [setup, setSetup] = useState<SetupStep | null>(null)
  const [dialog, setDialog] = useState<'regenerate' | 'disable' | null>(null)
  const [codes, setCodes] = useState<string[] | null>(null)
  const refresh = () => queryClient.invalidateQueries({ queryKey: accountKeys.security })

  let body
  if (!twoFactor.available) {
    body = (
      <Banner role="note" tone="info" title="Not available on this server.">
        Two-factor authentication needs an encryption key on the server. Ask whoever runs this server to add one.
      </Banner>
    )
  } else if (!security.hasPassword) {
    body = <p className="aw-quiet">Two-factor protects password sign-in. You sign in with single sign-on, which has its own.</p>
  } else if (twoFactor.enabled) {
    body = (
      <div className="aw-card aw-profile">
        <header className="aw-profile__head">
          <span className="aw-row__tile">
            <Icon name="lock" size={16} />
          </span>
          <div className="aw-profile__titles">
            <h3 className="aw-profile__name">Authenticator app</h3>
            <p className="aw-profile__sub">
              {twoFactor.enabledAt ? (
                <span title={formatFullDate(twoFactor.enabledAt)}>On since {formatShortDate(twoFactor.enabledAt)}. </span>
              ) : null}
              {countLabel(twoFactor.recoveryCodesLeft, 'recovery code')} left.
            </p>
          </div>
          <StatusChip icon="circle-check" tone="success">
            On
          </StatusChip>
        </header>
        {twoFactor.recoveryCodesLeft <= 3 && (
          <Banner tone="warning">
            {twoFactor.recoveryCodesLeft === 0
              ? 'You have no recovery codes left. Make new ones now so you don’t get locked out.'
              : `You have only ${countLabel(twoFactor.recoveryCodesLeft, 'recovery code')} left. Make new ones so you don’t get locked out.`}
          </Banner>
        )}
        <div className="aw-onetime__actions">
          <Button icon="retry" onClick={() => setDialog('regenerate')}>
            New recovery codes
          </Button>
          <Button onClick={() => setDialog('disable')} variant="ghost">
            Turn off
          </Button>
        </div>
      </div>
    )
  } else if (setup) {
    body = (
      <TwoFactorSetupFlow
        loginName={loginName}
        onCancel={() => setSetup(null)}
        onConfirmed={(recoveryCodes) => {
          setSetup({ step: 'codes', codes: recoveryCodes })
          void refresh()
        }}
        onStarted={(started) => setSetup({ step: 'code', setup: started })}
        state={setup}
      />
    )
  } else {
    body = (
      <div className="aw-card aw-profile">
        <header className="aw-profile__head">
          <span className="aw-row__tile">
            <Icon name="lock" size={16} />
          </span>
          <div className="aw-profile__titles">
            <h3 className="aw-profile__name">Authenticator app</h3>
            <p className="aw-profile__sub">
              {twoFactor.pending
                ? 'Setup was started but not finished.'
                : 'After your password, sign-in asks for a code from an app such as 1Password or Google Authenticator.'}
            </p>
          </div>
          <StatusChip icon="circle-minus">Off</StatusChip>
        </header>
        <div>
          <Button icon="shield-check" onClick={() => setSetup({ step: 'password' })} variant="primary">
            {twoFactor.pending ? 'Finish setting up' : 'Set up two-factor'}
          </Button>
        </div>
      </div>
    )
  }

  const shownCodes = codes ?? (setup?.step === 'codes' ? setup.codes : null)

  return (
    <Section id="two-factor-heading" title="Two-factor authentication">
      {shownCodes ? (
        <RecoveryCodes
          codes={shownCodes}
          loginName={loginName}
          onDone={() => {
            setCodes(null)
            setSetup(null)
          }}
        />
      ) : (
        body
      )}
      <CodeDialog
        description="Enter a code from your authenticator app. Your old recovery codes stop working."
        needsPassword={false}
        onClose={() => setDialog(null)}
        onSubmit={async ({ code }) => {
          const result = await regenerateRecoveryCodes(code)
          setDialog(null)
          setCodes(result.recoveryCodes)
          void refresh()
        }}
        open={dialog === 'regenerate'}
        submitLabel="Make new codes"
        title="New recovery codes"
      />
      <CodeDialog
        danger
        description="Sign-in will only ask for your password. Enter your password and a code from your authenticator app."
        needsPassword
        onClose={() => setDialog(null)}
        onSubmit={async ({ code, password }) => {
          await disableTwoFactor(password, code)
          setDialog(null)
          showToast({ tone: 'success', title: 'Two-factor authentication is off' })
          void refresh()
        }}
        open={dialog === 'disable'}
        submitLabel="Turn off"
        title="Turn off two-factor?"
      />
    </Section>
  )
}

function groupsOfFour(secret: string) {
  return secret.replace(/\s+/g, '').match(/.{1,4}/g)?.join(' ') ?? secret
}

function TwoFactorSetupFlow({
  loginName,
  onCancel,
  onConfirmed,
  onStarted,
  state,
}: {
  loginName: string
  onCancel: () => void
  onConfirmed: (codes: string[]) => void
  onStarted: (setup: TwoFactorSetup) => void
  state: SetupStep
}) {
  const [password, setPassword] = useState('')
  const [code, setCode] = useState('')
  const [error, setError] = useState<string | null>(null)
  const start = useMutation({
    mutationFn: () => startTwoFactorSetup(password),
    onSuccess: (setup) => {
      setPassword('')
      setError(null)
      onStarted(setup)
    },
    onError: (reason) => {
      setPassword('')
      setError(codeError(reason))
    },
  })
  const confirm = useMutation({
    mutationFn: () => confirmTwoFactor(code.trim().replace(/\s+/g, '')),
    onSuccess: (result) => onConfirmed(result.recoveryCodes),
    onError: (reason) => {
      setCode('')
      setError(codeError(reason))
    },
  })

  if (state.step === 'codes') return null
  if (state.step === 'password') {
    return (
      <form
        className="aw-card aw-formcard aw-form"
        onSubmit={(event) => {
          event.preventDefault()
          if (password) start.mutate()
        }}
      >
        <div>
          <h3 className="aw-formcard__title">Confirm it’s you</h3>
          <p className="aw-formcard__desc">Enter your password to start setting up two-factor authentication.</p>
        </div>
        <input autoComplete="username" hidden readOnly value={loginName} />
        <TextField
          autoComplete="current-password"
          autoFocus
          id="two-factor-password"
          label="Password"
          onChange={(event) => setPassword(event.target.value)}
          required
          type="password"
          value={password}
        />
        {error && <Banner tone="danger">{error}</Banner>}
        <div className="aw-form__actions">
          <Button onClick={onCancel}>Cancel</Button>
          <Button busy={start.isPending} disabled={!password} type="submit" variant="primary">
            Continue
          </Button>
        </div>
      </form>
    )
  }

  const { setup } = state
  return (
    <form
      className="aw-card aw-formcard aw-form"
      onSubmit={(event) => {
        event.preventDefault()
        if (code.trim()) confirm.mutate()
      }}
    >
      <div>
        <h3 className="aw-formcard__title">Add it to your authenticator app</h3>
        <p className="aw-formcard__desc">
          {setup.qrSvg ? 'Scan this code with the app. ' : null}
          On this device, <a href={setup.otpauthUri}>open it in your authenticator app</a>. Elsewhere, add an account by
          hand with this key:
        </p>
      </div>
      {setup.qrSvg ? (
        <img
          alt="QR code for your authenticator app"
          className="aw-twofactor-qr"
          height={200}
          src={`data:image/svg+xml;charset=utf-8,${encodeURIComponent(setup.qrSvg)}`}
          width={200}
        />
      ) : null}
      <dl className="aw-dl">
        <div className="aw-dl__row">
          <dt>Account</dt>
          <dd>
            {setup.issuer} · {setup.accountName}
          </dd>
        </div>
        <div className="aw-dl__row">
          <dt>Key</dt>
          <dd>
            <code aria-label={`Setup key ${setup.secret.split('').join(' ')}`} className="aw-secret-key">
              {groupsOfFour(setup.secret)}
            </code>
          </dd>
        </div>
        <div className="aw-dl__row">
          <dt>Type</dt>
          <dd>Time based, 6 digits</dd>
        </div>
      </dl>
      <TextField
        autoComplete="one-time-code"
        autoFocus
        hint="Enter the 6-digit code the app shows now."
        id="two-factor-confirm-code"
        inputMode="numeric"
        label="Code"
        maxLength={10}
        mono
        onChange={(event) => setCode(event.target.value)}
        required
        value={code}
      />
      {error && <Banner tone="danger">{error}</Banner>}
      <div className="aw-form__actions">
        <Button onClick={onCancel}>Cancel</Button>
        <Button busy={confirm.isPending} disabled={!code.trim()} type="submit" variant="primary">
          Turn on
        </Button>
      </div>
    </form>
  )
}

function RecoveryCodes({ codes, loginName, onDone }: { codes: string[]; loginName: string; onDone: () => void }) {
  const [copied, setCopied] = useState<'yes' | 'failed' | null>(null)
  const copy = () => {
    if (!navigator.clipboard) {
      setCopied('failed')
      return
    }
    navigator.clipboard.writeText(text).then(
      () => setCopied('yes'),
      () => setCopied('failed'),
    )
  }
  const text = codes.join('\n')
  const download = () => {
    const url = URL.createObjectURL(new Blob([`Agent Workspace recovery codes for ${loginName}\n\n${text}\n`], { type: 'text/plain' }))
    const link = document.createElement('a')
    link.href = url
    link.download = 'agent-workspace-recovery-codes.txt'
    link.click()
    window.setTimeout(() => URL.revokeObjectURL(url), 1000)
  }
  return (
    <aside aria-labelledby="recovery-codes-heading" className="aw-card aw-token">
      <div className="aw-token__head">
        <Icon name="key" size={18} />
        <h3 id="recovery-codes-heading">Save your recovery codes</h3>
      </div>
      <p>
        If you lose your phone, each of these signs you in once instead of a code. Keep them somewhere safe, like a password
        manager. They won’t be shown again.
      </p>
      <ol aria-label="Recovery codes" className="aw-recovery-codes">
        {codes.map((code) => (
          <li key={code}>
            <code>{code}</code>
          </li>
        ))}
      </ol>
      <div className="aw-onetime__actions">
        <Button icon={copied === 'yes' ? 'check' : 'copy'} onClick={copy} variant="primary">
          {copied === 'yes' ? 'Copied' : 'Copy codes'}
        </Button>
        <Button onClick={download}>
          Download
        </Button>
        <Button onClick={onDone}>I saved them</Button>
      </div>
      {copied === 'failed' && (
        <p className="aw-quiet" role="status">
          Couldn’t copy. Select the codes and copy them yourself, or download them.
        </p>
      )}
    </aside>
  )
}

function CodeDialog({
  danger = false,
  description,
  needsPassword,
  onClose,
  onSubmit,
  open,
  submitLabel,
  title,
}: {
  danger?: boolean
  description: string
  needsPassword: boolean
  onClose: () => void
  onSubmit: (input: { code: string; password: string }) => Promise<void>
  open: boolean
  submitLabel: string
  title: string
}) {
  const [password, setPassword] = useState('')
  const [code, setCode] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const id = useId()
  const close = () => {
    setPassword('')
    setCode('')
    setError(null)
    onClose()
  }
  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!code.trim() || (needsPassword && !password)) return
    setBusy(true)
    setError(null)
    try {
      await onSubmit({ code: code.trim().replace(/\s+/g, ''), password })
      setPassword('')
      setCode('')
    } catch (reason) {
      setCode('')
      setError(codeError(reason))
    } finally {
      setBusy(false)
    }
  }
  return (
    <Dialog
      description={description}
      footer={
        <>
          <Button onClick={close}>Cancel</Button>
          <Button busy={busy} disabled={!code.trim() || (needsPassword && !password)} type="submit" variant={danger ? 'danger' : 'primary'}>
            {submitLabel}
          </Button>
        </>
      }
      dirty={Boolean(password || code)}
      onClose={close}
      onSubmit={(event) => void submit(event)}
      open={open}
      title={title}
    >
      <div className="aw-form">
        {needsPassword && (
          <TextField
            autoComplete="current-password"
            id={`${id}-password`}
            label="Password"
            onChange={(event) => setPassword(event.target.value)}
            type="password"
            value={password}
          />
        )}
        <TextField
          autoComplete="one-time-code"
          id={`${id}-code`}
          inputMode="numeric"
          label="Code from your authenticator app"
          mono
          onChange={(event) => setCode(event.target.value)}
          value={code}
        />
        {error && <Banner tone="danger">{error}</Banner>}
      </div>
    </Dialog>
  )
}

function SessionsSection() {
  const queryClient = useQueryClient()
  const sessions = useQuery({ queryKey: accountKeys.sessions, queryFn: listSessions })
  const refresh = () => queryClient.invalidateQueries({ queryKey: accountKeys.sessions })
  const revokeOne = useMutation({
    mutationFn: (session: AccountSession) => revokeSession(session.id),
    onSuccess: () => showToast({ tone: 'success', title: 'Signed out that session' }),
    onError: (error) => {
      toastError('Couldn’t sign out that session', error)
    },
    onSettled: refresh,
  })
  const revokeOthers = useMutation({
    mutationFn: revokeOtherSessions,
    onSuccess: ({ revoked }) =>
      showToast({
        tone: 'success',
        title: `Signed out ${countLabel(revoked, 'other session')}`,
      }),
    onError: (error) => {
      toastError('Couldn’t sign out the other sessions', error)
    },
    onSettled: refresh,
  })
  const others = sessions.data?.sessions.filter((candidate) => !candidate.current) ?? []

  return (
    <Section
      actions={
        others.length > 0 ? (
          <Button busy={revokeOthers.isPending} icon="log-out" onClick={() => revokeOthers.mutate()} size="sm">
            Sign out all other sessions
          </Button>
        ) : null
      }
      description="Everywhere you’re signed in right now. Sign out anything you don’t recognize."
      id="sessions-heading"
      title="Sessions"
    >
      {sessions.isPending ? (
        <ListSkeleton label="Loading sessions" rows={2} />
      ) : sessions.isError ? (
        <Banner onRetry={() => void sessions.refetch()} title="Sessions didn’t load." tone="danger">
          {describeLoadError(sessions.error)}
        </Banner>
      ) : (
        <RowList label="Sessions">
          {[...sessions.data.sessions]
            .sort((left, right) => Number(right.current) - Number(left.current) || right.lastSeenAt.localeCompare(left.lastSeenAt))
            .map((candidate) => (
              <Row
                actions={
                  candidate.current ? null : (
                    <Button
                      busy={revokeOne.isPending && revokeOne.variables?.id === candidate.id}
                      disabled={revokeOne.isPending}
                      onClick={() => revokeOne.mutate(candidate)}
                      size="sm"
                      variant="ghost"
                    >
                      Sign out
                    </Button>
                  )
                }
                key={candidate.id}
                leading={
                  <span className="aw-row__tile">
                    <Icon name="lock" size={16} />
                  </span>
                }
                meta={
                  <span className="aw-row__metaline aw-wrap">
                    <span>{SIGN_IN_METHODS[candidate.signInMethod] ?? 'Signed in'}</span>
                    <span title={formatFullDate(candidate.lastSeenAt)}>
                      {candidate.current ? 'Active now' : `Last seen ${formatRelative(candidate.lastSeenAt)}`}
                    </span>
                    <span title={formatFullDate(candidate.createdAt)}>Signed in {formatShortDate(candidate.createdAt)}</span>
                  </span>
                }
                title={
                  <span className="aw-row__titleline" title={candidate.userAgent ?? undefined}>
                    {describeDevice(candidate.userAgent) ?? 'Unknown device'}
                    {candidate.current && <StatusChip tone="success">This device</StatusChip>}
                  </span>
                }
              />
            ))}
        </RowList>
      )}
    </Section>
  )
}

function ActivitySection({ events }: { events: AccountSecurity['recentEvents'] }) {
  return (
    <Section description="Recent sign-ins and security changes on your account." id="security-activity-heading" title="Recent activity">
      {events.length ? (
        <RowList label="Recent security activity">
          {events.map((event) => (
            <Row
              actions={
                event.outcome === 'succeeded' ? null : (
                  <StatusChip icon="triangle-alert" tone={event.outcome === 'denied' ? 'warning' : 'danger'}>
                    {event.outcome === 'denied' ? 'Denied' : 'Failed'}
                  </StatusChip>
                )
              }
              key={event.id}
              meta={
                <span title={formatFullDate(event.occurredAt)}>
                  {formatRelative(event.occurredAt)}
                  {event.clientName ? ` · ${event.clientName}` : event.channel === 'browser' ? ' · Browser' : ''}
                </span>
              }
              title={actionPhrase(event.action) ? actionSentence(event.action) : <span title={event.action}>{actionSentence(event.action)}</span>}
            />
          ))}
        </RowList>
      ) : (
        <p className="aw-quiet">Nothing yet.</p>
      )}
    </Section>
  )
}
