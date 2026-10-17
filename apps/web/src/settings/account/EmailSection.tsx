import { useState, type FormEvent } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { requestJson } from '../../api/client'
import { describeError, toastError } from '../../app/mutations'
import { Banner, Button, Section, SettingsSkeleton, StatusChip, TextField, formatRelative, showToast } from '../../ui'
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

/** Profile › Email address: where password resets and email notifications go, once confirmed. */
export function EmailSection() {
  const queryClient = useQueryClient()
  const email = useQuery({ queryKey: emailKey, queryFn: getEmail })
  const [draft, setDraft] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
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
      if (!next.verified) showToast({ tone: 'success', title: 'Check your inbox', description: `We sent a confirmation link to ${next.address}.` })
    },
    onError: (reason) => setError(describeError(reason).message),
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
    onSuccess: () => settle(undefined),
    onError: (reason) => toastError('The address wasn’t removed', reason),
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
        <Banner action={<Button onClick={() => void email.refetch()} size="sm">Try again</Button>} tone="danger">
          Your email address didn’t load.
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
              {!current.verified && current.confirmationSentAt
                ? `Link sent ${formatRelative(current.confirmationSentAt)}. Open it from your inbox to confirm.`
                : null}
            </p>
          )}
          {!current?.mailAvailable && !current?.address ? (
            <p className="aw-quiet">None of your workspaces send email yet. A workspace owner sets it up in Workspace › Email.</p>
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
              <Button busy={remove.isPending} onClick={() => remove.mutate()} size="sm" variant="ghost">
                Remove
              </Button>
            )}
          </div>
          {error && <Banner tone="warning">{error}</Banner>}
        </>
      )}
    </Section>
  )
}
