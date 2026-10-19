import { useState, type FormEvent } from 'react'
import { Link } from 'react-router-dom'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { requestJson } from '../../api/client'
import { describeError, toastError } from '../../app/mutations'
import { Banner, Button, ConfirmDialog, Section, SettingsSkeleton, StatusChip, TextField, describeLoadError, formatFullDate, formatRelative, showToast } from '../../ui'
import { workspaceSettingsHref } from '../paths'
import { notificationKeys } from './usageApi'

export type AccountEmail = {
  address: string | null
  verified: boolean
  confirmationSentAt: string | null
  mailAvailable: boolean
}

const emailKey = ['account-email'] as const

const getEmail = () => requestJson<AccountEmail>('/api/v1/account/email')
const setEmail = (address: string) => requestJson<AccountEmail>('/api/v1/account/email', { method: 'PUT', json: { address } })
const removeEmail = () => requestJson<void>('/api/v1/account/email', { method: 'DELETE' })
const resend = () => requestJson<AccountEmail>('/api/v1/account/email/resend', { method: 'POST', json: {} })

/**
 * Profile › Email address: where password resets and email notifications go,
 * once confirmed. `canSetUpEmail` links the owner of the workspace to the page
 * where its email is set up.
 */
export function EmailSection({ canSetUpEmail = false }: { canSetUpEmail?: boolean }) {
  const queryClient = useQueryClient()
  const email = useQuery({ queryKey: emailKey, queryFn: getEmail })
  const [draft, setDraft] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [confirmRemove, setConfirmRemove] = useState(false)
  const settle = (next: AccountEmail | undefined) => {
    setError(null)
    setDraft(null)
    if (next) queryClient.setQueryData(emailKey, next)
    else void queryClient.invalidateQueries({ queryKey: emailKey })
    void queryClient.invalidateQueries({ queryKey: notificationKeys.settings })
  }
  const save = useMutation({
    mutationFn: (address: string) => setEmail(address),
    onSuccess: (next) => {
      settle(next)
      if (!next.verified) showToast({ tone: 'success', title: 'Check your inbox', description: `Confirmation link sent to ${next.address}.` })
    },
    onError: (reason) => setError(`The address wasn’t saved. ${describeError(reason).message}`),
  })
  const again = useMutation({
    mutationFn: resend,
    onSuccess: (next) => {
      settle(next)
      showToast({ tone: 'success', title: 'Confirmation link sent again' })
    },
    onError: (reason) => toastError('The link wasn’t sent', reason),
  })
  const remove = useMutation({
    mutationFn: removeEmail,
    onSuccess: () => {
      setConfirmRemove(false)
      settle(undefined)
      showToast({ tone: 'success', title: 'Email address removed' })
    },
  })

  const current = email.data
  const value = draft ?? current?.address ?? ''
  const trimmed = value.trim()
  const submit = (event: FormEvent) => {
    event.preventDefault()
    if (trimmed && trimmed !== current?.address) save.mutate(trimmed)
  }

  return (
    <Section
      description="Password reset links and the notifications you choose to get by email go here, once you confirm it."
      id="email-address-heading"
      title="Email address"
    >
      {email.isPending ? (
        <SettingsSkeleton label="Loading your email address" sections={1} />
      ) : email.isError ? (
        <Banner onRetry={() => void email.refetch()} title="Your email address didn’t load." tone="danger">
          {describeLoadError(email.error)}
        </Banner>
      ) : (
        <>
          {current?.address && (
            <p className="aw-quiet">
              {current.verified ? (
                <StatusChip icon="check" tone="success">Confirmed</StatusChip>
              ) : (
                <StatusChip tone="warning">Waiting for confirmation</StatusChip>
              )}{' '}
              {!current.verified && current.confirmationSentAt ? (
                <>
                  Link sent <span title={formatFullDate(current.confirmationSentAt)}>{formatRelative(current.confirmationSentAt)}</span>.
                  Open it from your inbox to confirm.
                </>
              ) : null}
            </p>
          )}
          {!current?.mailAvailable && !current?.address ? (
            <p className="aw-quiet">
              None of your workspaces send email yet.{' '}
              {canSetUpEmail ? (
                <>
                  Set it up in <Link to={workspaceSettingsHref('email')}>Workspace › Email</Link>.
                </>
              ) : (
                'A workspace owner sets it up in Workspace › Email.'
              )}
            </p>
          ) : (
            <form className="aw-inline-form" onSubmit={submit}>
              <TextField
                autoComplete="email"
                id="account-email"
                inputMode="email"
                label="Email address"
                maxLength={254}
                onChange={(event) => setDraft(event.target.value)}
                type="email"
                value={value}
              />
              <Button
                busy={save.isPending}
                disabled={!trimmed || trimmed === current?.address || !current?.mailAvailable}
                type="submit"
                variant="primary"
              >
                {current?.address ? 'Change' : 'Add'}
              </Button>
            </form>
          )}
          <div className="aw-form__row">
            {current?.address && !current.verified && current.mailAvailable && (
              <Button busy={again.isPending} onClick={() => again.mutate()} size="sm">
                Send the link again
              </Button>
            )}
            {current?.address && (
              <Button onClick={() => setConfirmRemove(true)} size="sm" variant="ghost">
                Remove
              </Button>
            )}
          </div>
          {error && <Banner tone="warning">{error}</Banner>}
          <ConfirmDialog
            busy={remove.isPending}
            confirmLabel="Remove address"
            error={remove.error ? `The address wasn’t removed. ${describeError(remove.error).message}` : null}
            onClose={() => {
              setConfirmRemove(false)
              remove.reset()
            }}
            onConfirm={() => remove.mutate()}
            open={confirmRemove}
            title={`Remove ${current?.address ?? 'your email address'}?`}
          >
            Password reset links and email notifications stop until you add an address again.
          </ConfirmDialog>
        </>
      )}
    </Section>
  )
}
