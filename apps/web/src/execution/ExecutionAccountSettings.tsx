import { useState, type FormEvent } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { isApiError } from '../api/errors'
import { queryKeys } from '../api/queryKeys'
import { humanize } from '../projects/format'
import { Banner, Button, EmptyState, ErrorState, Icon, MessageSkeleton, StatusChip, TextField } from '../ui'
import {
  cancelAccountLogin,
  createPersonalExecutionProfile,
  getAccountStatus,
  listExecutionProfiles,
  loginWithApiKey,
  logoutAccount,
  startAccountLogin,
  type AccountStatus,
  type LoginCeremony,
} from './api'

export function ExecutionAccountSettings({ actorId, projectId }: { actorId: string; projectId: string }) {
  const queryClient = useQueryClient()
  const profiles = useQuery({
    queryKey: queryKeys.executionProfiles(projectId),
    queryFn: () => listExecutionProfiles(projectId),
  })
  const createProfile = useMutation({
    mutationFn: () => createPersonalExecutionProfile(projectId, crypto.randomUUID()),
    onSuccess: (profile) => {
      queryClient.setQueryData<Awaited<ReturnType<typeof listExecutionProfiles>>>(
        queryKeys.executionProfiles(projectId),
        (current = []) => [...current.filter((item) => item.id !== profile.id), profile],
      )
    },
  })

  if (profiles.isPending) return <MessageSkeleton label="Loading your Codex account" rows={1} />
  if (profiles.isError) {
    return (
      <ErrorState action={<Button onClick={() => void profiles.refetch()}>Try again</Button>} title="Your Codex account didn’t load">
        Check your connection and try again.
      </ErrorState>
    )
  }
  const owned = profiles.data.filter((profile) => profile.owner.id === actorId)
  if (!owned.length) {
    return (
      <div className="aw-card aw-formcard">
        <EmptyState
          action={
            <Button busy={createProfile.isPending} icon="plus" onClick={() => createProfile.mutate()} variant="primary">
              Create personal profile
            </Button>
          }
          icon="terminal"
          title="Set up Codex for your runs"
        >
          Create a personal Codex profile before starting runs. It’s never shared with another Project member.
        </EmptyState>
        {createProfile.isError && (
          <Banner tone="danger">
            {isApiError(createProfile.error) ? createProfile.error.message : 'Your Codex profile wasn’t created. Try again.'}
          </Banner>
        )}
      </div>
    )
  }

  return (
    <div className="aw-settings-stack">
      {owned.map((profile) => (
        <ExecutionProfileAccount
          key={profile.id}
          name={profile.name}
          profileId={profile.id}
          projectId={projectId}
          provider={`${profile.provider.kind} ${profile.provider.binaryVersion}`}
        />
      ))}
    </div>
  )
}

function AccountState({ state }: { state: string | undefined }) {
  if (!state) return <StatusChip icon="loader" spin>Checking</StatusChip>
  if (state === 'authenticated') {
    return (
      <StatusChip icon="circle-check" tone="success">
        Signed in
      </StatusChip>
    )
  }
  if (state === 'unauthenticated') return <StatusChip icon="circle-minus">Signed out</StatusChip>
  return <StatusChip tone="warning">{humanize(state)}</StatusChip>
}

function ExecutionProfileAccount({
  name,
  profileId,
  projectId,
  provider,
}: {
  name: string
  profileId: string
  projectId: string
  provider: string
}) {
  const queryClient = useQueryClient()
  const accountKey = queryKeys.executionProfileAccount(projectId, profileId)
  const account = useQuery({
    queryKey: accountKey,
    queryFn: () => getAccountStatus(projectId, profileId),
    refetchInterval: (query) => (query.state.data?.ceremony?.state === 'pending' ? 2_000 : false),
  })
  const [apiKey, setApiKey] = useState('')
  const [error, setError] = useState<string | null>(null)
  const update = (value: AccountStatus) => queryClient.setQueryData(accountKey, value)
  const ceremonyUpdate = (ceremony: LoginCeremony) =>
    queryClient.setQueryData<AccountStatus>(accountKey, (current) => (current ? { ...current, ceremony } : current))
  const login = useMutation({
    mutationFn: (kind: 'browser' | 'device-code') => startAccountLogin(projectId, profileId, kind, crypto.randomUUID()),
    onSuccess: ceremonyUpdate,
    onError: (reason) => setError(errorMessage(reason)),
  })
  const apiKeyLogin = useMutation({
    mutationFn: (value: string) => loginWithApiKey(projectId, profileId, value, crypto.randomUUID()),
    onSuccess: () => void account.refetch(),
    onError: (reason) => setError(errorMessage(reason)),
  })
  const cancel = useMutation({
    mutationFn: () => cancelAccountLogin(projectId, profileId, crypto.randomUUID()),
    onSuccess: update,
    onError: (reason) => setError(errorMessage(reason)),
  })
  const logout = useMutation({
    mutationFn: () => logoutAccount(projectId, profileId, crypto.randomUUID()),
    onSuccess: update,
    onError: (reason) => setError(errorMessage(reason)),
  })
  const submitApiKey = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    const value = apiKey
    setApiKey('')
    setError(null)
    if (value) apiKeyLogin.mutate(value)
  }
  const pending = login.isPending || apiKeyLogin.isPending || cancel.isPending || logout.isPending
  const status = account.data
  const ceremony = status?.ceremony

  return (
    <article aria-labelledby={`profile-${profileId}`} className="aw-card aw-profile">
      <header className="aw-profile__head">
        <span className="aw-row__tile">
          <Icon name="terminal" size={16} />
        </span>
        <div className="aw-profile__titles">
          <h3 className="aw-profile__name" id={`profile-${profileId}`}>
            {name}
          </h3>
          <p className="aw-profile__sub">{provider}</p>
        </div>
        <AccountState state={status?.accountState} />
      </header>
      {account.isError && <Banner tone="danger">We couldn’t check this account right now.</Banner>}
      {error && <Banner tone="danger">{error}</Banner>}
      {(status?.account || status?.rateLimits?.primary) && (
        <div className="aw-profile__facts">
          {status?.account && (
            <p>
              Signed in with {status.account.accountType}
              {status.account.email ? ` as ${status.account.email}` : ''}.
            </p>
          )}
          {status?.rateLimits?.primary && (
            <div className="aw-meter">
              <span className="aw-meter__label">Usage this period</span>
              <progress className="aw-meter__bar" max={100} value={status.rateLimits.primary.usedPercent} />
              <span className="aw-meter__value">{status.rateLimits.primary.usedPercent}%</span>
            </div>
          )}
        </div>
      )}
      {ceremony?.authUrl && (
        <a className="aw-btn aw-btn--primary aw-btn--md aw-profile__cta" href={ceremony.authUrl} rel="noreferrer" target="_blank">
          <span className="aw-btn__label">Continue secure browser sign-in</span>
          <Icon name="external" />
        </a>
      )}
      {ceremony?.verificationUrl && (
        <div className="aw-devicecode">
          <p>
            Open the sign-in page and enter this code:
          </p>
          <code className="aw-devicecode__code">{ceremony.userCode}</code>
          <a className="aw-btn aw-btn--secondary aw-btn--md" href={ceremony.verificationUrl} rel="noreferrer" target="_blank">
            <span className="aw-btn__label">Open device sign-in</span>
            <Icon name="external" />
          </a>
        </div>
      )}
      <footer className="aw-profile__actions">
        {ceremony?.state === 'pending' ? (
          <Button disabled={pending} onClick={() => cancel.mutate()} variant="ghost">
            Cancel sign-in
          </Button>
        ) : status?.accountState === 'authenticated' ? (
          <Button disabled={pending} icon="log-out" onClick={() => logout.mutate()} variant="ghost">
            Sign out of Codex
          </Button>
        ) : (
          <>
            <div className="aw-profile__options">
              <Button disabled={pending} icon="external" onClick={() => login.mutate('browser')} variant="primary">
                Sign in with browser
              </Button>
              <Button disabled={pending} onClick={() => login.mutate('device-code')}>
                Use device code
              </Button>
            </div>
            <p className="aw-quiet">Or use an API key. It goes straight to your Codex profile and is never shown again.</p>
            <form className="aw-inline-form" onSubmit={submitApiKey}>
              <TextField
                autoComplete="off"
                id={`api-key-${profileId}`}
                label="OpenAI API key"
                onChange={(event) => setApiKey(event.target.value)}
                type="password"
                value={apiKey}
              />
              <Button disabled={pending || !apiKey} icon="key" type="submit">
                Use API key
              </Button>
            </form>
          </>
        )}
      </footer>
    </article>
  )
}

function errorMessage(reason: unknown) {
  return isApiError(reason) ? reason.message : 'That didn’t work. Try again.'
}
