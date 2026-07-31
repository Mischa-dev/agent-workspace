import { FormEvent, useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { isApiError } from '../api/errors'
import { queryKeys } from '../api/queryKeys'
import {
  cancelAccountLogin,
  getAccountStatus,
  listExecutionProfiles,
  loginWithApiKey,
  logoutAccount,
  startAccountLogin,
  type AccountStatus,
  type LoginCeremony,
} from './api'

export function ExecutionAccountSettings({
  actorId,
  projectId,
}: {
  actorId: string
  projectId: string
}) {
  const profiles = useQuery({
    queryKey: queryKeys.executionProfiles(projectId),
    queryFn: () => listExecutionProfiles(projectId),
  })

  if (profiles.isPending) return <p>Loading execution profiles…</p>
  if (profiles.isError) return <p role="alert">Execution profiles could not be loaded.</p>
  const owned = profiles.data.filter((profile) => profile.owner.id === actorId)
  if (!owned.length) return <p>No personal execution profile is assigned to this account.</p>

  return (
    <div className="execution-profile-list">
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
    refetchInterval: (query) =>
      query.state.data?.ceremony?.state === 'pending' ? 2_000 : false,
  })
  const [apiKey, setApiKey] = useState('')
  const [error, setError] = useState<string | null>(null)
  const update = (value: AccountStatus) => queryClient.setQueryData(accountKey, value)
  const ceremonyUpdate = (ceremony: LoginCeremony) =>
    queryClient.setQueryData<AccountStatus>(accountKey, (current) =>
      current ? { ...current, ceremony } : current,
    )
  const login = useMutation({
    mutationFn: (kind: 'browser' | 'device-code') =>
      startAccountLogin(projectId, profileId, kind, crypto.randomUUID()),
    onSuccess: ceremonyUpdate,
    onError: (reason) => setError(errorMessage(reason)),
  })
  const apiKeyLogin = useMutation({
    mutationFn: (value: string) =>
      loginWithApiKey(projectId, profileId, value, crypto.randomUUID()),
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

  return (
    <article className="execution-profile-card">
      <header>
        <div>
          <h3>{name}</h3>
          <p>{provider}</p>
        </div>
        <span className="status-chip" data-state={status?.accountState ?? 'unknown'}>
          {(status?.accountState ?? 'checking').replaceAll('_', ' ')}
        </span>
      </header>
      {account.isPending && <p>Checking provider account…</p>}
      {account.isError && <p role="alert">Provider account status is unavailable.</p>}
      {error && <p role="alert">{error}</p>}
      {status?.account && (
        <p>
          Signed in with {status.account.accountType}
          {status.account.email ? ` as ${status.account.email}` : ''}.
        </p>
      )}
      {status?.rateLimits?.primary && (
        <p>Primary allowance used: {status.rateLimits.primary.usedPercent}%.</p>
      )}
      {status?.ceremony?.authUrl && (
        <p>
          <a href={status.ceremony.authUrl} rel="noreferrer" target="_blank">
            Continue secure browser sign-in
          </a>
        </p>
      )}
      {status?.ceremony?.verificationUrl && (
        <div className="device-code-instructions">
          <a href={status.ceremony.verificationUrl} rel="noreferrer" target="_blank">
            Open device sign-in
          </a>
          <p>Enter code <strong>{status.ceremony.userCode}</strong>.</p>
        </div>
      )}
      {status?.ceremony?.state === 'pending' ? (
        <button disabled={pending} onClick={() => cancel.mutate()} type="button">
          Cancel sign-in
        </button>
      ) : status?.accountState === 'authenticated' ? (
        <button className="danger-button" disabled={pending} onClick={() => logout.mutate()} type="button">
          Sign out provider
        </button>
      ) : (
        <div className="execution-login-options">
          <button disabled={pending} onClick={() => login.mutate('browser')} type="button">
            Sign in with browser
          </button>
          <button disabled={pending} onClick={() => login.mutate('device-code')} type="button">
            Use device code
          </button>
          <form onSubmit={submitApiKey}>
            <div className="field">
              <label htmlFor={`api-key-${profileId}`}>OpenAI API key</label>
              <input
                autoComplete="off"
                id={`api-key-${profileId}`}
                onChange={(event) => setApiKey(event.target.value)}
                type="password"
                value={apiKey}
              />
            </div>
            <button disabled={pending || !apiKey} type="submit">Use API key</button>
          </form>
        </div>
      )}
    </article>
  )
}

function errorMessage(reason: unknown) {
  return isApiError(reason) ? reason.message : 'The provider account operation failed.'
}
