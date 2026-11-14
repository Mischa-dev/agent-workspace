import { useState, type FormEvent } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { describeError, toastError } from '../../app/mutations'
import {
  Banner,
  Button,
  ConfirmDialog,
  ErrorState,
  PageHeader,
  Section,
  SelectField,
  SettingsSkeleton,
  StatusChip,
  Switch,
  TextField,
  formatFullDate,
  formatRelative,
  showToast,
} from '../../ui'
import type { SettingsPageProps } from '../types'
import {
  type MailSecurity,
  type MailSettings,
  mailKeys,
  getMailSettings,
  removeMailSettings,
  saveMailSettings,
  sendTestEmail,
} from './mailApi'
import '../../styles/pages.css'

const DEFAULT_PORTS: Record<MailSecurity, number> = { starttls: 587, tls: 465, none: 25 }

/** Workspace › Email: how this workspace sends mail (password resets, address confirmations, notifications). Owners only. */
export default function EmailPage({ workspace }: SettingsPageProps) {
  const organizationId = workspace?.id ?? ''
  const settings = useQuery({
    queryKey: mailKeys.settings(organizationId),
    queryFn: () => getMailSettings(organizationId),
    enabled: Boolean(organizationId),
  })
  if (!workspace) return null
  return (
    <>
      <PageHeader icon="message" title="Email">
        How {workspace.name} sends email: password reset links, address confirmations and the notifications people choose
        to get by email. Use your mail provider’s SMTP details.
      </PageHeader>
      {settings.isPending ? (
        <SettingsSkeleton label="Loading email settings" sections={2} />
      ) : settings.isError ? (
        <ErrorState error={settings.error} onRetry={() => void settings.refetch()} title="Email settings didn’t load" />
      ) : (
        <>
          <SmtpSection key={settings.data.updatedAt ?? 'new'} organizationId={organizationId} settings={settings.data} />
          {settings.data.configured && <TestSection organizationId={organizationId} settings={settings.data} />}
          {settings.data.configured && <RemoveSection organizationId={organizationId} />}
        </>
      )}
    </>
  )
}

function SmtpSection({ organizationId, settings }: { organizationId: string; settings: MailSettings }) {
  const queryClient = useQueryClient()
  const [host, setHost] = useState(settings.smtp?.host ?? '')
  const [security, setSecurity] = useState<MailSecurity>(settings.smtp?.security ?? 'starttls')
  const [port, setPort] = useState(String(settings.smtp?.port ?? DEFAULT_PORTS.starttls))
  const [username, setUsername] = useState(settings.smtp?.username ?? '')
  const [password, setPassword] = useState('')
  const [fromAddress, setFromAddress] = useState(settings.fromAddress ?? '')
  const [fromName, setFromName] = useState(settings.fromName ?? 'Agent Workspace')
  const [enabled, setEnabled] = useState(settings.configured ? settings.enabled : true)
  const [error, setError] = useState<string | null>(null)
  const save = useMutation({
    mutationFn: () =>
      saveMailSettings(organizationId, {
        provider: 'smtp',
        smtp: { host: host.trim(), port: Number(port), security, username: username.trim() || null },
        fromAddress: fromAddress.trim(),
        fromName: fromName.trim() || null,
        enabled,
        ...(password ? { password } : {}),
        ...(!username.trim() && settings.passwordSet ? { clearPassword: true } : {}),
      }),
    onSuccess: (next) => {
      setError(null)
      setPassword('')
      queryClient.setQueryData(mailKeys.settings(organizationId), next)
      showToast({ tone: 'success', title: 'Email settings saved', description: 'Send a test email to check them.' })
    },
    onError: (reason) => setError(describeSaveError(reason)),
  })
  const portNumber = Number(port)
  const valid = host.trim() && Number.isInteger(portNumber) && portNumber > 0 && portNumber < 65536 && fromAddress.includes('@')
  const submit = (event: FormEvent) => {
    event.preventDefault()
    if (valid) save.mutate()
  }
  const changeSecurity = (next: MailSecurity) => {
    if (Number(port) === DEFAULT_PORTS[security]) setPort(String(DEFAULT_PORTS[next]))
    setSecurity(next)
  }
  return (
    <Section
      description="The address mail comes from must be one your provider lets you send as. The password is stored encrypted and never shown again."
      id="smtp-heading"
      title="Outgoing mail (SMTP)"
    >
      <form className="aw-form" onSubmit={submit}>
        <TextField autoComplete="off" hint="Such as smtp.fastmail.com or smtp.gmail.com" id="smtp-host" label="Server" maxLength={253} onChange={(event) => setHost(event.target.value)} required spellCheck={false} value={host} />
        <div className="aw-form__row">
          <SelectField id="smtp-security" label="Security" onChange={(event) => changeSecurity(event.target.value as MailSecurity)} value={security}>
            <option value="starttls">STARTTLS (usually port 587)</option>
            <option value="tls">TLS (usually port 465)</option>
            <option value="none">None (a relay on this server only)</option>
          </SelectField>
          <TextField id="smtp-port" inputMode="numeric" label="Port" maxLength={5} onChange={(event) => setPort(event.target.value.replace(/\D/g, ''))} required value={port} />
        </div>
        <div className="aw-form__row">
          <TextField autoComplete="off" hint="Leave empty for a relay that needs no sign-in." id="smtp-username" label="Username" maxLength={254} onChange={(event) => setUsername(event.target.value)} spellCheck={false} value={username} />
          <TextField
            autoComplete="new-password"
            hint={settings.passwordSet ? 'Saved. Type a new one to replace it.' : 'An app password if your provider has them.'}
            id="smtp-password"
            label="Password"
            maxLength={1024}
            onChange={(event) => setPassword(event.target.value)}
            placeholder={settings.passwordSet ? '••••••••' : undefined}
            type="password"
            value={password}
          />
        </div>
        <div className="aw-form__row">
          <TextField autoComplete="off" id="smtp-from" inputMode="email" label="Send from" maxLength={254} onChange={(event) => setFromAddress(event.target.value)} placeholder="workspace@example.com" required spellCheck={false} type="email" value={fromAddress} />
          <TextField id="smtp-from-name" label="Sender name" maxLength={120} onChange={(event) => setFromName(event.target.value)} value={fromName} />
        </div>
        <Switch checked={enabled} label="Send email from this workspace" onChange={setEnabled} />
        {error && <Banner tone="warning">{error}</Banner>}
        <div className="aw-form__row">
          <Button busy={save.isPending} disabled={!valid} type="submit" variant="primary">
            Save
          </Button>
        </div>
      </form>
    </Section>
  )
}

function describeSaveError(reason: unknown) {
  const { message } = describeError(reason)
  if (message.toLowerCase().includes('security')) return 'Without TLS, the server has to be on this machine (localhost). Choose STARTTLS or TLS.'
  return message
}

function TestSection({ organizationId, settings }: { organizationId: string; settings: MailSettings }) {
  const queryClient = useQueryClient()
  const [to, setTo] = useState(settings.yourAddress ?? '')
  const test = useMutation({
    mutationFn: () => sendTestEmail(organizationId, to.trim() || null),
    onSuccess: (result) => {
      void queryClient.invalidateQueries({ queryKey: mailKeys.settings(organizationId) })
      if (result.ok) showToast({ tone: 'success', title: 'Test email sent', description: `Check ${result.sentTo}.` })
    },
    onError: (reason) => toastError('The test didn’t run', reason),
  })
  const last = test.data
    ? { ok: test.data.ok, error: test.data.error, at: null }
    : settings.lastTest
  return (
    <Section description="Sends a short message now and shows what the mail server said." id="test-email-heading" title="Send a test email">
      <form
        className="aw-inline-form"
        onSubmit={(event) => {
          event.preventDefault()
          test.mutate()
        }}
      >
        <TextField id="test-email-to" inputMode="email" label="Send to" maxLength={254} onChange={(event) => setTo(event.target.value)} required type="email" value={to} />
        <Button busy={test.isPending} disabled={!to.includes('@') || !settings.enabled} type="submit">
          Send test email
        </Button>
      </form>
      {!settings.enabled && <p className="aw-quiet">Sending is turned off above.</p>}
      {last &&
        (last.ok ? (
          <p className="aw-quiet" role="status">
            <StatusChip icon="check" tone="success">Worked</StatusChip>{' '}
            {last.at ? <span title={formatFullDate(last.at)}>Last test {formatRelative(last.at)}.</span> : 'The mail server accepted it.'}
          </p>
        ) : (
          <Banner role="status" title="The test didn’t go through" tone="warning">
            {last.error ?? 'The mail server didn’t accept it.'}
          </Banner>
        ))}
    </Section>
  )
}

function RemoveSection({ organizationId }: { organizationId: string }) {
  const queryClient = useQueryClient()
  const [confirming, setConfirming] = useState(false)
  const remove = useMutation({
    mutationFn: () => removeMailSettings(organizationId),
    onSuccess: () => {
      setConfirming(false)
      void queryClient.invalidateQueries({ queryKey: mailKeys.settings(organizationId) })
      showToast({ tone: 'success', title: 'Email settings removed' })
    },
  })
  return (
    <Section description="Stops all email from this workspace and forgets the password." id="remove-email-heading" title="Remove">
      <Button onClick={() => setConfirming(true)} variant="danger">
        Remove email settings
      </Button>
      <ConfirmDialog
        busy={remove.isPending}
        confirmLabel="Remove email settings"
        error={remove.isError ? `The settings weren’t removed. ${describeError(remove.error).message}` : null}
        onClose={() => {
          setConfirming(false)
          remove.reset()
        }}
        onConfirm={() => remove.mutate()}
        open={confirming}
        title="Remove email settings?"
      >
        People won’t be able to reset their password by email, and email notifications stop until an owner sets it up again.
      </ConfirmDialog>
    </Section>
  )
}
