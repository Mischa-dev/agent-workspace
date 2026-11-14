import { useEffect, useRef, useState, type FormEvent } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Link } from 'react-router-dom'

import { queryKeys } from '../api/queryKeys'
import { humanize } from '../projects/format'
import { projectSettingsHref } from '../settings/paths'
import {
  Banner,
  Button,
  ConfirmDialog,
  EmptyState,
  ErrorState,
  Icon,
  SelectField,
  SettingsSkeleton,
  StatusChip,
  TextField,
  describeError,
  showToast,
} from '../ui'
import {
  cancelAccountLogin,
  createProviderAccount,
  deleteProviderAccount,
  getAccountStatus,
  listHarnesses,
  listProviderAccounts,
  loginWithApiKey,
  logoutAccount,
  pickableModels,
  startAccountLogin,
  updateProviderAccount,
  useProviderAccount as setAccountInUse,
  type AccountStatus,
  type Harness,
  type LoginCeremony,
  type ProviderAccount,
  type RunSettings,
} from './api'
import { RunSettingsFields } from './RunSettingsFields'
import '../styles/pages.css'

/** Your accounts for runs: the sign-ins your runs use. An account is yours
 * alone and works in every project you choose to use it in, so you sign in
 * once. Everything shown comes from each harness's manifest. */
export function ExecutionAccountSettings({ projectId }: { actorId?: string; projectId: string }) {
  const queryClient = useQueryClient()
  const harnesses = useQuery({ queryKey: queryKeys.harnesses(projectId), queryFn: () => listHarnesses(projectId) })
  const accounts = useQuery({
    queryKey: queryKeys.providerAccounts(projectId),
    queryFn: () => listProviderAccounts(projectId),
  })
  const [adding, setAdding] = useState('')
  const refresh = () => void queryClient.invalidateQueries({ queryKey: queryKeys.providerAccounts(projectId) })
  // A new account starts signed out, so it's only used here at once when it's
  // the first one; otherwise the project keeps the account that works.
  const add = useMutation({
    mutationFn: async (kind: string) => {
      const existing = accounts.data ?? []
      const harnessName = harnesses.data?.find((harness) => harness.kind === kind)?.displayName
      const name = harnessName ? unusedName(harnessName, existing) : undefined
      const account = await createProviderAccount(projectId, kind, crypto.randomUUID(), name)
      if (existing.length === 0) await setAccountInUse(projectId, account.id, crypto.randomUUID())
      return account
    },
    onSuccess: (account) => {
      refresh()
      showToast({ tone: 'success', title: `${account.name} added`, description: 'Sign in to it below to use it for runs.' })
    },
  })

  if (harnesses.isPending || accounts.isPending) return <SettingsSkeleton label="Loading your accounts" />
  if (harnesses.isError || accounts.isError) {
    return (
      <ErrorState
        error={harnesses.error ?? accounts.error}
        onRetry={() => {
          void harnesses.refetch()
          void accounts.refetch()
        }}
        title="Your accounts didn’t load"
      />
    )
  }
  const byKind = new Map(harnesses.data.map((harness) => [harness.kind, harness]))
  const addable = harnesses.data
  const chosen = adding || addable[0]?.kind || ''
  const addForm = addable.length ? (
    <div className="aw-inline-form">
      {addable.length > 1 ? (
        <SelectField id="add-account-harness" label="Harness" onChange={(event) => setAdding(event.target.value)} value={chosen}>
          {addable.map((harness) => (
            <option key={harness.kind} value={harness.kind}>
              {harness.displayName}
            </option>
          ))}
        </SelectField>
      ) : null}
      <Button busy={add.isPending} disabled={!chosen} icon="plus" onClick={() => add.mutate(chosen)} variant={accounts.data.length ? 'secondary' : 'primary'}>
        {accounts.data.length ? 'Add another account' : `Connect ${byKind.get(chosen)?.displayName ?? 'an account'}`}
      </Button>
    </div>
  ) : null

  return (
    <div className="aw-settings-stack">
      {accounts.data.length === 0 ? (
        <div className="aw-card aw-formcard">
          <EmptyState action={addForm} icon="terminal" title="Connect an account for your runs">
            Runs use your own account with the agent’s provider. Once you sign in, you can use it in any of your projects.
          </EmptyState>
        </div>
      ) : (
        accounts.data.map((account) => (
          <ProviderAccountCard account={account} harness={byKind.get(account.adapterKind)} key={account.id} onChange={refresh} projectId={projectId} />
        ))
      )}
      {accounts.data.length > 0 ? addForm : null}
      {add.isError && <Banner tone="danger">The account wasn’t added. {errorMessage(add.error)}</Banner>}
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
  if (state === 'logged_out' || state === 'authentication_required') return <StatusChip icon="circle-minus">Signed out</StatusChip>
  return <StatusChip tone="warning">{humanize(state)}</StatusChip>
}

function ProviderAccountCard({
  account,
  harness,
  onChange,
  projectId,
}: {
  account: ProviderAccount
  harness: Harness | undefined
  onChange: () => void
  projectId: string
}) {
  const queryClient = useQueryClient()
  const statusKey = queryKeys.providerAccountStatus(projectId, account.id)
  const status = useQuery({
    queryKey: statusKey,
    queryFn: () => getAccountStatus(projectId, account.id),
    refetchInterval: (query) => (query.state.data?.ceremony?.state === 'pending' ? 2_000 : false),
  })
  // A sign-in finishing while polling changes the account (and its version)
  // on the server; reload the list so the defaults form saves against it.
  const polledState = status.data?.accountState
  const lastPolledState = useRef(polledState)
  useEffect(() => {
    const previous = lastPolledState.current
    lastPolledState.current = polledState
    if (previous !== undefined && polledState !== undefined && previous !== polledState) onChange()
  }, [polledState, onChange])
  const [apiKey, setApiKey] = useState('')
  const [error, setError] = useState<string | null>(null)
  const update = (value: AccountStatus) => {
    queryClient.setQueryData(statusKey, value)
    onChange()
  }
  const ceremonyUpdate = (ceremony: LoginCeremony) =>
    queryClient.setQueryData<AccountStatus>(statusKey, (current) => (current ? { ...current, ceremony } : current))
  const login = useMutation({
    mutationFn: (kind: 'browser' | 'device_code') => startAccountLogin(projectId, account.id, kind, crypto.randomUUID()),
    onSuccess: ceremonyUpdate,
    onError: (reason) => setError(errorMessage(reason)),
  })
  const apiKeyLogin = useMutation({
    mutationFn: (value: string) => loginWithApiKey(projectId, account.id, value, crypto.randomUUID()),
    onSuccess: () => {
      void status.refetch()
      onChange()
    },
    onError: (reason) => setError(errorMessage(reason)),
  })
  const cancel = useMutation({
    mutationFn: () => cancelAccountLogin(projectId, account.id, crypto.randomUUID()),
    onSuccess: update,
    onError: (reason) => setError(errorMessage(reason)),
  })
  const logout = useMutation({
    mutationFn: () => logoutAccount(projectId, account.id, crypto.randomUUID()),
    onSuccess: update,
    onError: (reason) => setError(errorMessage(reason)),
  })
  const use = useMutation({
    mutationFn: () => setAccountInUse(projectId, account.id, crypto.randomUUID()),
    onSuccess: onChange,
    onError: (reason) => setError(errorMessage(reason)),
  })
  const [confirmingDelete, setConfirmingDelete] = useState(false)
  const remove = useMutation({
    mutationFn: () => deleteProviderAccount(projectId, account.id),
    onSuccess: () => {
      setConfirmingDelete(false)
      onChange()
      showToast({ tone: 'success', title: `${account.name} deleted` })
    },
  })
  const [renaming, setRenaming] = useState<string | null>(null)
  const rename = useMutation({
    mutationFn: (name: string) =>
      updateProviderAccount(projectId, account.id, { expectedVersion: account.version, name }, crypto.randomUUID()),
    onSuccess: (updated) => {
      setRenaming(null)
      setError(null)
      onChange()
      showToast({ tone: 'success', title: `Renamed to ${updated.name}` })
    },
    onError: (reason) => setError(`The account wasn’t renamed. ${errorMessage(reason)}`),
  })
  const submitApiKey = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    const value = apiKey
    setApiKey('')
    setError(null)
    if (value) apiKeyLogin.mutate(value)
  }
  const pending = login.isPending || apiKeyLogin.isPending || cancel.isPending || logout.isPending || use.isPending || rename.isPending
  const current = status.data
  const ceremony = current?.ceremony
  const methods = harness?.loginMethods ?? []
  const harnessName = harness?.displayName ?? account.harnessName ?? account.adapterKind
  const apiKeyMethod = methods.find((method) => method.kind === 'api_key')

  return (
    <article aria-labelledby={`account-${account.id}`} className="aw-card aw-profile">
      <header className="aw-profile__head">
        <span className="aw-row__tile">
          <Icon name="terminal" size={16} />
        </span>
        <div className="aw-profile__titles">
          <h3 className="aw-profile__name" id={`account-${account.id}`}>
            {account.name}
          </h3>
          {renaming !== null ? (
            <form
              className="aw-inline-form"
              onSubmit={(event) => {
                event.preventDefault()
                const name = renaming.trim()
                if (name && name !== account.name) rename.mutate(name)
                else setRenaming(null)
              }}
            >
              <TextField
                autoFocus
                compact
                id={`account-name-${account.id}`}
                label="Name"
                maxLength={120}
                onChange={(event) => setRenaming(event.target.value)}
                value={renaming}
              />
              <Button busy={rename.isPending} disabled={!renaming.trim()} size="sm" type="submit" variant="primary">
                Save
              </Button>
              <Button disabled={rename.isPending} onClick={() => setRenaming(null)} size="sm">
                Cancel
              </Button>
            </form>
          ) : null}
          <p className="aw-profile__sub">
            {harnessName}
            {harness?.runsOn === 'cloud' ? ' · runs in the cloud' : ''}
            {account.activeRuns ? ` · ${account.activeRuns} running` : ''}
          </p>
        </div>
        {account.preferred ? <StatusChip icon="check" tone="success">Used here</StatusChip> : null}
        <AccountState state={current?.accountState ?? account.accountState} />
      </header>
      {!harness && <Banner tone="warning">This server no longer has this harness installed, so it can’t run.</Banner>}
      {status.isError && (
        <Banner onRetry={() => void status.refetch()} tone="danger">
          This account’s sign-in couldn’t be checked.
        </Banner>
      )}
      {error && <Banner tone="danger">{error}</Banner>}
      {(current?.account || current?.rateLimits?.primary) && (
        <div className="aw-profile__facts">
          {current?.account && (
            <p>
              Signed in with {current.account.accountType}
              {current.account.email ? ` as ${current.account.email}` : ''}.
            </p>
          )}
          {current?.rateLimits?.primary && (
            <div className="aw-meter">
              <span className="aw-meter__label">Usage this period</span>
              <progress className="aw-meter__bar" max={100} value={current.rateLimits.primary.usedPercent} />
              <span className="aw-meter__value">{current.rateLimits.primary.usedPercent}%</span>
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
          <p>Open the sign-in page and enter this code:</p>
          <code className="aw-devicecode__code">{ceremony.userCode}</code>
          <a className="aw-btn aw-btn--secondary aw-btn--md" href={ceremony.verificationUrl} rel="noreferrer" target="_blank">
            <span className="aw-btn__label">Open device sign-in</span>
            <Icon name="external" />
          </a>
        </div>
      )}
      {harness && current?.accountState === 'authenticated' ? (
        <AccountRunDefaults account={account} harness={harness} onSaved={onChange} projectId={projectId} />
      ) : null}
      <footer className="aw-profile__actions">
        {!account.preferred ? (
          <Button disabled={pending || !harness} onClick={() => use.mutate()}>
            Use in this project
          </Button>
        ) : null}
        {renaming === null ? (
          <Button disabled={pending} icon="pencil" onClick={() => setRenaming(account.name)} variant="ghost">
            Rename
          </Button>
        ) : null}
        <Button disabled={pending || account.activeRuns > 0} icon="trash" onClick={() => setConfirmingDelete(true)} variant="ghost">
          Delete
        </Button>
        {ceremony?.state === 'pending' ? (
          <Button disabled={pending} onClick={() => cancel.mutate()} variant="ghost">
            Cancel sign-in
          </Button>
        ) : current?.accountState === 'authenticated' ? (
          <Button disabled={pending} icon="log-out" onClick={() => logout.mutate()} variant="ghost">
            Sign out
          </Button>
        ) : (
          <>
            <div className="aw-profile__options">
              {methods
                .filter((method) => method.kind !== 'api_key')
                .map((method, index) => (
                  <Button
                    disabled={pending}
                    icon={method.kind === 'browser' ? 'external' : undefined}
                    key={method.kind}
                    onClick={() => login.mutate(method.kind as 'browser' | 'device_code')}
                    variant={index === 0 ? 'primary' : 'secondary'}
                  >
                    {method.label}
                  </Button>
                ))}
            </div>
            {apiKeyMethod ? (
              <>
                <p className="aw-quiet">Or use an API key. It goes straight to this account and is never shown again.</p>
                <form className="aw-inline-form" onSubmit={submitApiKey}>
                  <TextField
                    autoComplete="off"
                    id={`api-key-${account.id}`}
                    label={apiKeyMethod.label}
                    onChange={(event) => setApiKey(event.target.value)}
                    type="password"
                    value={apiKey}
                  />
                  <Button disabled={pending || !apiKey} icon="key" type="submit">
                    Use API key
                  </Button>
                </form>
              </>
            ) : null}
          </>
        )}
      </footer>
      <ConfirmDialog
        busy={remove.isPending}
        confirmLabel="Delete account"
        error={remove.error ? describeError(remove.error).message : null}
        onClose={() => {
          setConfirmingDelete(false)
          remove.reset()
        }}
        onConfirm={() => remove.mutate()}
        open={confirmingDelete}
        title={`Delete ${account.name}?`}
      >
        It’s signed out and its files on this server go, in every project that uses it. Runs it already made keep its name.
      </ConfirmDialog>
    </article>
  )
}

/** The model, reasoning level and speed this account's runs use unless a run
 * picks its own, and how many of its runs may work at once. */
function AccountRunDefaults({
  account,
  harness,
  onSaved,
  projectId,
}: {
  account: ProviderAccount
  harness: Harness
  onSaved: () => void
  projectId: string
}) {
  const [draft, setDraft] = useState<RunSettings>(account.runDefaults)
  const [limit, setLimit] = useState(account.maxConcurrentRuns)
  const save = useMutation({
    mutationFn: () =>
      updateProviderAccount(
        projectId,
        account.id,
        { expectedVersion: account.version, maxConcurrentRuns: limit, runDefaults: clean(draft) },
        crypto.randomUUID(),
      ),
    onSuccess: onSaved,
  })
  const changed = JSON.stringify(clean(draft)) !== JSON.stringify(clean(account.runDefaults)) || limit !== account.maxConcurrentRuns
  const limits = Array.from({ length: Math.max(1, harness.concurrency.maxPerAccount) }, (_, index) => index + 1)

  return (
    <div className="aw-profile__facts">
      <RunSettingsFields harnesses={[harness]} idPrefix={`defaults-${account.id}`} onChange={setDraft} value={draft} />
      {pickableModels(harness).length > 0 ? (
        <p className="aw-quiet">
          Used only where the launch, the agent and the{' '}
          <Link to={projectSettingsHref(projectId, 'runs')}>project’s and workspace’s settings</Link> leave a value unset.
        </p>
      ) : null}
      {harness.features.concurrentRuns ? (
        <SelectField
          id={`limit-${account.id}`}
          label="Runs at once"
          onChange={(event) => setLimit(Number(event.target.value))}
          value={String(limit)}
        >
          {limits.map((value) => (
            <option key={value} value={value}>
              {value}
            </option>
          ))}
        </SelectField>
      ) : null}
      {save.isError && <Banner tone="danger">The defaults weren’t saved. {errorMessage(save.error)}</Banner>}
      {changed ? (
        <Button busy={save.isPending} onClick={() => save.mutate()} size="sm" variant="primary">
          Save defaults
        </Button>
      ) : null}
    </div>
  )
}

function clean(settings: RunSettings): RunSettings {
  return Object.fromEntries(Object.entries(settings).filter(([, entry]) => entry !== undefined && entry !== '')) as RunSettings
}

function errorMessage(reason: unknown) {
  return describeError(reason).message
}

/**
 * A name for a new account when the harness's own name (the server's default)
 * is taken: "<Harness> account 2" and on. Undefined leaves the default.
 */
function unusedName(harnessName: string, accounts: ProviderAccount[]) {
  const taken = new Set(accounts.map((account) => account.name.trim().toLocaleLowerCase()))
  if (!taken.has(harnessName.toLocaleLowerCase())) return undefined
  for (let number = 2; ; number += 1) {
    const name = `${harnessName} account ${number}`
    if (!taken.has(name.toLocaleLowerCase())) return name
  }
}
