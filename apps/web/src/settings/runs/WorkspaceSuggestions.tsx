import { useState, type FormEvent } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { aiKeys, getWorkspaceAi, providerKeyName, testWorkspaceAi, type ProviderManifest, type WorkspaceAi } from '../../ai/api'
import { aiErrorMessage } from '../../ai/links'
import { describeError } from '../../app/mutations'
import { Banner, Button, Dialog, ErrorState, Icon, Row, RowList, Section, SettingsSkeleton, StatusChip, TextField, showToast } from '../../ui'
import { useEffectiveSettings, type SettingsTarget } from '../api'
import { ScopedSettings } from '../ScopedSettings'
import { putWorkspaceSecret, workspaceKeys } from '../workspace/api'
import { SUGGESTION_KEYS, SUGGESTION_UNSET_LABELS, here } from './options'
import './runs.css'

const dollars = (micros: number) => `$${(micros / 1_000_000).toFixed(micros >= 10_000_000 ? 0 : 2)}`

/**
 * The workspace's suggestions and summaries: whether they're ready, the
 * providers and their keys, the settings, and this month's spend. Only owners
 * and admins see the keys and the spend.
 */
export function WorkspaceSuggestions({ manager, organizationId }: { manager: boolean; organizationId: string }) {
  const target: SettingsTarget = { scope: 'workspace', organizationId }
  const status = useQuery({
    queryKey: aiKeys.workspace(organizationId),
    queryFn: () => getWorkspaceAi(organizationId),
    enabled: Boolean(organizationId) && manager,
  })
  const effective = useEffectiveSettings(target)
  const providerId = effective.data?.settings.find((setting) => setting.key === 'ai.provider')?.value
  const provider = status.data?.providers.find((candidate) => candidate.id === providerId)
  const settings = (
    <ScopedSettings
      options={
        status.data
          ? {
              'ai.provider': status.data.providers.map((candidate) => ({
                value: candidate.id,
                label: candidate.testingOnly ? `${candidate.displayName} (testing only)` : candidate.displayName,
              })),
            }
          : undefined
      }
      prefixes={SUGGESTION_KEYS}
      readOnlyNote={false}
      target={target}
      unsetLabels={{
        ...SUGGESTION_UNSET_LABELS,
        ...(provider
          ? {
              'ai.model': `The provider’s default (${provider.defaultQuickModel})`,
              'ai.deep_model': `The provider’s default (${provider.defaultDeepModel})`,
              ...(provider.defaultBaseUrl ? { 'ai.base_url': provider.defaultBaseUrl } : {}),
            }
          : {}),
      }}
    />
  )
  return (
    <Section
      description="The provider and models behind dependency detection, triage, plan import and summaries. Projects can choose their own."
      id="suggestions"
      title="Suggestions and summaries"
    >
      {!manager ? (
        settings
      ) : status.isPending ? (
        <SettingsSkeleton label="Loading suggestions and summaries" sections={2} />
      ) : status.isError ? (
        <ErrorState action={<Button onClick={() => void status.refetch()}>Try again</Button>} title="Suggestions and summaries didn’t load">
          Check your connection and try again.
        </ErrorState>
      ) : (
        <>
          <ReadyBanner organizationId={organizationId} status={status.data} />
          <Providers organizationId={organizationId} status={status.data} />
          {settings}
          <Spend status={status.data} />
        </>
      )}
    </Section>
  )
}

function ReadyBanner({ organizationId, status }: { organizationId: string; status: WorkspaceAi }) {
  const test = useMutation({
    mutationFn: () => testWorkspaceAi(organizationId),
    onSuccess: (result) => showToast({ tone: 'success', title: `${result.model} answered in ${result.latencyMs} ms` }),
  })
  const providerName = status.providers.find((provider) => provider.id === status.provider)?.displayName ?? status.provider
  return (
    <>
      {status.configured ? (
        <Banner
          action={
            <Button busy={test.isPending} onClick={() => test.mutate()} size="sm">
              Test connection
            </Button>
          }
          icon="circle-check"
          title="Ready."
          tone="success"
        >
          Quick jobs use {status.quickModel} from {providerName}.
        </Banner>
      ) : (
        <Banner icon="bot" role="note" title="Not set up yet." tone="info">
          {status.problem ? here(status.problem) : 'Choose a provider below and add its key.'}
        </Banner>
      )}
      {test.isError ? <Banner tone="danger">{aiErrorMessage(test.error, 'The test')}</Banner> : null}
    </>
  )
}

function Providers({ organizationId, status }: { organizationId: string; status: WorkspaceAi }) {
  const queryClient = useQueryClient()
  const [keyFor, setKeyFor] = useState<ProviderManifest | null>(null)
  return (
    <div className="aw-subsection">
      <div>
        <h3 className="aw-subsection__title">Providers</h3>
        <p className="aw-quiet">Keys are encrypted on the server and never shown again.</p>
      </div>
      <RowList label="Providers">
        {status.providers.map((provider) => (
          <Row
            actions={
              provider.needsKey ? (
                <Button icon="key" onClick={() => setKeyFor(provider)} size="sm" variant="ghost">
                  {status.workspaceKeys[provider.id] ? 'Replace key' : 'Add key'}
                </Button>
              ) : null
            }
            key={provider.id}
            leading={
              <span className="aw-row__tile">
                <Icon name="bot" size={16} />
              </span>
            }
            meta={
              <>
                <span className="aw-runs__line">{provider.description}</span>
                <span className="aw-row__metaline aw-wrap">
                  <span>
                    Quick jobs: <code className="aw-inline-code">{provider.defaultQuickModel}</code>
                  </span>
                  <span>
                    Deep jobs: <code className="aw-inline-code">{provider.defaultDeepModel}</code>
                  </span>
                </span>
              </>
            }
            title={
              <span className="aw-row__titleline">
                {provider.displayName}
                {status.provider === provider.id ? <StatusChip tone="success">In use</StatusChip> : null}
                {provider.needsKey ? (
                  status.workspaceKeys[provider.id] ? (
                    <StatusChip icon="key">Key saved</StatusChip>
                  ) : (
                    <StatusChip tone="warning">No key</StatusChip>
                  )
                ) : null}
                {provider.testingOnly ? <StatusChip>Testing only</StatusChip> : null}
              </span>
            }
          />
        ))}
      </RowList>
      <ProviderKeyDialog
        onClose={() => setKeyFor(null)}
        onSaved={() => {
          setKeyFor(null)
          void queryClient.invalidateQueries({ queryKey: aiKeys.workspace(organizationId) })
          void queryClient.invalidateQueries({ queryKey: workspaceKeys.secrets(organizationId) })
        }}
        organizationId={organizationId}
        provider={keyFor}
        replacing={keyFor ? status.workspaceKeys[keyFor.id] === true : false}
      />
    </div>
  )
}

function Spend({ status }: { status: WorkspaceAi }) {
  const budgetMicros = status.budgetUsd * 1_000_000
  const share = budgetMicros > 0 ? Math.min(1, status.spentMicros / budgetMicros) : 0
  const maxDay = Math.max(1, ...status.usageByDay.map((day) => day.costMicros))
  return (
    <div className="aw-subsection">
      <h3 className="aw-subsection__title">This month</h3>
      {status.spentMicros === 0 && !status.usageByModel.length ? (
        <p className="aw-quiet">
          Nothing spent this month yet. {budgetMicros > 0 ? `The monthly budget is ${dollars(budgetMicros)}.` : 'There’s no monthly limit.'}
        </p>
      ) : (
        <div className="aw-ai-budget">
          <p>
            <strong>{dollars(status.spentMicros)}</strong> spent
            {budgetMicros > 0 ? ` of the ${dollars(budgetMicros)} monthly budget` : ' (no budget limit)'}.
            {budgetMicros > 0 && share >= 1 ? ' Suggestions and summaries are paused until next month or a higher budget.' : ''}
          </p>
          {budgetMicros > 0 ? <progress aria-label="Budget used" className="aw-progress__bar" max={1} value={share} /> : null}
        </div>
      )}
      {status.usageByModel.length ? (
        <div aria-label="Spend by model" className="aw-ai-usage__scroll" role="region" tabIndex={0}>
          <table className="aw-ai-usage">
            <thead>
              <tr>
                <th scope="col">Model</th>
                <th scope="col">Calls</th>
                <th scope="col">Tokens in / out</th>
                <th scope="col">Cost</th>
              </tr>
            </thead>
            <tbody>
              {status.usageByModel.map((row) => (
                <tr key={`${row.provider}:${row.model}`}>
                  <td>
                    <code className="aw-inline-code">{row.model}</code>
                  </td>
                  <td>{row.calls}</td>
                  <td>
                    {row.inputTokens.toLocaleString()} / {row.outputTokens.toLocaleString()}
                  </td>
                  <td>{dollars(row.costMicros)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      ) : null}
      {status.usageByDay.length ? (
        <ol aria-label="Cost per day, last 30 days" className="aw-ai-days">
          {status.usageByDay.map((day) => (
            <li key={day.day} title={`${day.day}: ${day.calls} calls, ${dollars(day.costMicros)}`}>
              <span className="aw-ai-days__bar" style={{ height: `${Math.max(4, (day.costMicros / maxDay) * 100)}%` }} />
              <span className="visually-hidden">
                {day.day}: {day.calls} calls, {dollars(day.costMicros)}
              </span>
            </li>
          ))}
        </ol>
      ) : null}
    </div>
  )
}

function ProviderKeyDialog({
  onClose,
  onSaved,
  organizationId,
  provider,
  replacing,
}: {
  onClose: () => void
  onSaved: () => void
  organizationId: string
  provider: ProviderManifest | null
  replacing: boolean
}) {
  const [value, setValue] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [shownFor, setShownFor] = useState<string | null>(null)
  if ((provider?.id ?? null) !== shownFor) {
    setShownFor(provider?.id ?? null)
    setValue('')
    setError(null)
  }
  const save = useMutation({
    mutationFn: () =>
      putWorkspaceSecret(organizationId, providerKeyName(provider?.id ?? ''), value, `${provider?.displayName ?? ''} API key for suggestions and summaries`),
    onSuccess: () => {
      setValue('')
      showToast({ tone: 'success', title: `${provider?.displayName} key saved` })
      onSaved()
    },
    onError: (reason) => setError(describeError(reason).message),
  })
  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!value || !provider) return
    save.mutate()
  }
  return (
    <Dialog
      description="Paste the key once. It’s encrypted on the server, never shown again, and only sent to this provider."
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button busy={save.isPending} disabled={!value} type="submit" variant="primary">
            {replacing ? 'Replace key' : 'Save key'}
          </Button>
        </>
      }
      onClose={onClose}
      onSubmit={submit}
      open={provider !== null}
      title={provider ? `${provider.displayName} API key` : 'API key'}
    >
      <div className="aw-form">
        <TextField
          autoComplete="new-password"
          data-autofocus
          id="ai-provider-key"
          label="Key"
          onChange={(event) => setValue(event.target.value)}
          spellCheck={false}
          type="password"
          value={value}
        />
        {error ? <Banner tone="danger">{error}</Banner> : null}
      </div>
    </Dialog>
  )
}
