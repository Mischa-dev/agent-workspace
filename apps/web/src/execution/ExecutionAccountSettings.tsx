import { useState, type FormEvent } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { isApiError } from '../api/errors'
import { queryKeys } from '../api/queryKeys'
import { humanize } from '../projects/format'
import { Banner, Button, EmptyState, ErrorState, Icon, MessageSkeleton, SelectField, StatusChip, TextField } from '../ui'
import {
  cancelAccountLogin,
  createProviderAccount,
  getAccountStatus,
  listHarnesses,
  listProviderAccounts,
  loginWithApiKey,
  logoutAccount,
  pickableModels,
  refreshCatalog,
  saveModel,
  startAccountLogin,
  updateProviderAccount,
  useProviderAccount as setAccountInUse,
  type AccountStatus,
  type Harness,
  type LoginCeremony,
  type ModelOverrides,
  type ProviderAccount,
  type RunSettings,
} from './api'

/** Your provider accounts: the sign-ins your runs use. An account is yours
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
  const add = useMutation({
    mutationFn: async (kind: string) => {
      const account = await createProviderAccount(projectId, kind, crypto.randomUUID())
      return setAccountInUse(projectId, account.id, crypto.randomUUID())
    },
    onSuccess: refresh,
  })

  if (harnesses.isPending || accounts.isPending) return <MessageSkeleton label="Loading your accounts" rows={1} />
  if (harnesses.isError || accounts.isError) {
    return (
      <ErrorState
        action={
          <Button
            onClick={() => {
              void harnesses.refetch()
              void accounts.refetch()
            }}
          >
            Try again
          </Button>
        }
        title="Your accounts didn’t load"
      >
        Check your connection and try again.
      </ErrorState>
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
            Runs use your own account with the agent’s provider. It’s never shared with anyone else, and once you sign in you can use it in any project.
          </EmptyState>
        </div>
      ) : (
        accounts.data.map((account) => (
          <ProviderAccountCard account={account} harness={byKind.get(account.adapterKind)} key={account.id} onChange={refresh} projectId={projectId} />
        ))
      )}
      {accounts.data.length > 0 ? addForm : null}
      {add.isError && <Banner tone="danger">{errorMessage(add.error)}</Banner>}
      {harnesses.data
        .filter((harness) => harness.canEditCatalog && harness.catalog.models.length > 0)
        .map((harness) => (
          <ModelCatalog harness={harness} key={harness.kind} projectId={projectId} />
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
  const submitApiKey = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    const value = apiKey
    setApiKey('')
    setError(null)
    if (value) apiKeyLogin.mutate(value)
  }
  const pending = login.isPending || apiKeyLogin.isPending || cancel.isPending || logout.isPending || use.isPending
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
      {status.isError && <Banner tone="danger">We couldn’t check this account right now.</Banner>}
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
      <RunSettingsFields harness={harness} idPrefix={`defaults-${account.id}`} onChange={setDraft} value={draft} />
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
      {save.isError && <Banner tone="danger">{errorMessage(save.error)}</Banner>}
      {changed ? (
        <Button busy={save.isPending} onClick={() => save.mutate()} size="sm" variant="primary">
          Save defaults
        </Button>
      ) : null}
    </div>
  )
}

/** Model, reasoning and speed pickers for one harness, drawn from its
 * catalog. An empty choice means "use the default". */
export function RunSettingsFields({
  harness,
  idPrefix,
  onChange,
  value,
}: {
  harness: Harness
  idPrefix: string
  onChange: (value: RunSettings) => void
  value: RunSettings
}) {
  const models = pickableModels(harness)
  const model = models.find((candidate) => candidate.id === value.model) ?? models.find((candidate) => candidate.isDefault)
  if (!models.length) return null
  return (
    <div className="aw-inline-form">
      <SelectField
        id={`${idPrefix}-model`}
        label="Model"
        onChange={(event) => onChange({ ...value, model: event.target.value || undefined, reasoning: undefined, speed: undefined })}
        value={value.model ?? ''}
      >
        <option value="">Default ({models.find((candidate) => candidate.isDefault)?.displayName ?? models[0].displayName})</option>
        {models.map((candidate) => (
          <option key={candidate.id} value={candidate.id}>
            {candidate.displayName}
          </option>
        ))}
      </SelectField>
      {model && model.reasoningLevels.length > 0 ? (
        <SelectField
          id={`${idPrefix}-reasoning`}
          label="Reasoning"
          onChange={(event) => onChange({ ...value, reasoning: event.target.value || undefined })}
          value={value.reasoning ?? ''}
        >
          <option value="">Default{model.defaultReasoning ? ` (${humanize(model.defaultReasoning)})` : ''}</option>
          {model.reasoningLevels.map((level) => (
            <option key={level} value={level}>
              {humanize(level)}
            </option>
          ))}
        </SelectField>
      ) : null}
      {model && model.speeds.length > 1 ? (
        <SelectField
          id={`${idPrefix}-speed`}
          label="Speed"
          onChange={(event) => onChange({ ...value, speed: event.target.value || undefined })}
          value={value.speed ?? ''}
        >
          <option value="">Default</option>
          {model.speeds.map((speed) => (
            <option key={speed.id} value={speed.id}>
              {speed.name}
            </option>
          ))}
        </SelectField>
      ) : null}
    </div>
  )
}

/** The workspace's model list for one harness. Admins hide models nobody
 * should pick, choose the default, and pull the provider's latest list. */
function ModelCatalog({ harness, projectId }: { harness: Harness; projectId: string }) {
  const queryClient = useQueryClient()
  const saved = (updated: Harness) =>
    queryClient.setQueryData<Harness[]>(queryKeys.harnesses(projectId), (current) =>
      current?.map((entry) => (entry.kind === updated.kind ? updated : entry)),
    )
  const edit = useMutation({
    mutationFn: ({ modelId, change }: { modelId: string; change: ModelOverrides }) =>
      saveModel(projectId, harness.kind, modelId, change, crypto.randomUUID()),
    onSuccess: saved,
  })
  const refresh = useMutation({
    mutationFn: () => refreshCatalog(projectId, harness.kind, crypto.randomUUID()),
    onSuccess: saved,
  })
  const failure = edit.error ?? refresh.error

  return (
    <section aria-labelledby={`catalog-${harness.kind}`} className="aw-card aw-profile">
      <header className="aw-profile__head">
        <div className="aw-profile__titles">
          <h3 className="aw-profile__name" id={`catalog-${harness.kind}`}>
            {harness.displayName} models
          </h3>
          <p className="aw-profile__sub">
            Everyone in this workspace picks from this list.
            {harness.catalog.refreshedAt ? ` Updated ${new Date(harness.catalog.refreshedAt).toLocaleDateString()}.` : ''}
          </p>
        </div>
        {harness.features.modelListing ? (
          <Button busy={refresh.isPending} icon="retry" onClick={() => refresh.mutate()} size="sm" variant="secondary">
            Refresh list
          </Button>
        ) : null}
      </header>
      {failure ? <Banner tone="danger">{errorMessage(failure)}</Banner> : null}
      <ul className="aw-rows">
        {harness.catalog.models.map((model) => (
          <li className="aw-row" key={model.id}>
            <span className="aw-row__main">
              <span className="aw-row__title">{model.displayName}</span>
              <span className="aw-quiet">
                {[model.isDefault && 'Default', model.hidden && 'Hidden', model.source === 'manual' && 'Added by hand'].filter(Boolean).join(' · ') || model.id}
              </span>
            </span>
            {!model.isDefault && !model.hidden ? (
              <Button disabled={edit.isPending} onClick={() => edit.mutate({ modelId: model.id, change: { ...model.overrides, isDefault: true } })} size="sm" variant="ghost">
                Make default
              </Button>
            ) : null}
            <Button
              disabled={edit.isPending || (model.isDefault && !model.hidden)}
              onClick={() => edit.mutate({ modelId: model.id, change: { ...model.overrides, hidden: !model.hidden } })}
              size="sm"
              variant="ghost"
            >
              {model.hidden ? `Show ${model.displayName}` : `Hide ${model.displayName}`}
            </Button>
          </li>
        ))}
      </ul>
    </section>
  )
}

function clean(settings: RunSettings): RunSettings {
  return Object.fromEntries(Object.entries(settings).filter(([, entry]) => entry !== undefined && entry !== '')) as RunSettings
}

function errorMessage(reason: unknown) {
  return isApiError(reason) ? reason.message : 'That didn’t work. Try again.'
}
