import { useState, type FormEvent } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { aiKeys, getWorkspaceAi, providerKeyName, testWorkspaceAi, type ProviderManifest, type WorkspaceAi } from '../../ai/api'
import { aiErrorMessage } from '../../ai/links'
import { describeError } from '../../app/mutations'
import { Banner, Button, Dialog, ErrorState, Icon, PageHeader, Row, RowList, Section, SettingsSkeleton, StatusChip, TextField, countLabel, showToast } from '../../ui'
import { ScopedSettings } from '../ScopedSettings'
import type { SettingsPageProps } from '../types'
import { putWorkspaceSecret, workspaceKeys } from './api'

const dollars = (micros: number) => `$${(micros / 1_000_000).toFixed(micros >= 10_000_000 ? 0 : 2)}`

/** Workspace › Models: the provider, its key, the models, the budget and what's been spent. */
export default function AiPage({ workspace }: SettingsPageProps) {
  const organizationId = workspace?.id ?? ''
  const manager = workspace?.role === 'owner' || workspace?.role === 'admin'
  const status = useQuery({
    queryKey: aiKeys.workspace(organizationId),
    queryFn: () => getWorkspaceAi(organizationId),
    enabled: Boolean(organizationId) && manager,
  })
  if (!workspace) return null
  const providerOptions = status.data?.providers.map((provider) => ({
    value: provider.id,
    label: provider.testingOnly ? `${provider.displayName} (testing only)` : provider.displayName,
  }))
  return (
    <>
      <PageHeader icon="star" title="Models">
        The provider and models behind dependency detection, triage, plan import and summaries in {workspace.name}. Any provider works;
        projects can choose their own.
      </PageHeader>
      {manager ? <WorkspaceStatus organizationId={organizationId} status={status} /> : null}
      <Section id="ai-settings-heading" title="Settings">
        <ScopedSettings
          options={providerOptions ? { 'ai.provider': providerOptions } : undefined}
          prefixes={['ai.']}
          target={{ scope: 'workspace', organizationId: workspace.id }}
        />
      </Section>
    </>
  )
}

function WorkspaceStatus({ organizationId, status }: { organizationId: string; status: ReturnType<typeof useQuery<WorkspaceAi>> }) {
  const queryClient = useQueryClient()
  const [keyFor, setKeyFor] = useState<ProviderManifest | null>(null)
  const test = useMutation({
    mutationFn: () => testWorkspaceAi(organizationId),
    onSuccess: (result) => showToast({ tone: 'success', title: `${result.model} answered in ${result.latencyMs} ms` }),
  })
  if (status.isPending) return <SettingsSkeleton label="Loading model status" sections={1} />
  if (status.isError) {
    return <ErrorState error={status.error} onRetry={() => void status.refetch()} title="Model status didn’t load" />
  }
  const data = status.data
  const budgetMicros = data.budgetUsd * 1_000_000
  const share = budgetMicros > 0 ? Math.min(1, data.spentMicros / budgetMicros) : 0
  const maxDay = Math.max(1, ...data.usageByDay.map((day) => day.costMicros))

  return (
    <>
      {data.configured ? (
        <Banner
          action={
            <Button busy={test.isPending} onClick={() => test.mutate()} size="sm">
              Test connection
            </Button>
          }
          icon="circle-check"
          title="Models are ready."
          tone="success"
        >
          Quick jobs use {data.quickModel} from {data.providers.find((provider) => provider.id === data.provider)?.displayName ?? data.provider}.
        </Banner>
      ) : (
        <Banner icon="bot" role="note" title="Models aren’t set up yet." tone="info">
          {data.problem ?? 'Choose a provider below and add its key.'}
        </Banner>
      )}
      {test.isError ? <Banner tone="danger">{aiErrorMessage(test.error, 'The test')}</Banner> : null}
      <Section description="Keys are encrypted on the server and never shown again." id="ai-providers-heading" title="Providers">
        <RowList label="Providers">
          {data.providers.map((provider) => (
            <Row
              actions={
                provider.needsKey ? (
                  <Button icon="key" onClick={() => setKeyFor(provider)} size="sm" variant="ghost">
                    {data.workspaceKeys[provider.id] ? 'Replace key' : 'Add key'}
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
                <span className="aw-row__metaline aw-wrap">
                  <span>{provider.description}</span>
                  <span>
                    Quick: <code className="aw-inline-code">{provider.defaultQuickModel}</code> · Deep:{' '}
                    <code className="aw-inline-code">{provider.defaultDeepModel}</code>
                  </span>
                </span>
              }
              title={
                <span className="aw-wrap">
                  {provider.displayName}{' '}
                  {data.provider === provider.id ? <StatusChip tone="success">In use</StatusChip> : null}
                  {provider.needsKey ? (
                    data.workspaceKeys[provider.id] ? (
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
      </Section>
      <Section id="ai-usage-heading" title="This month">
        <div className="aw-ai-budget">
          <p>
            <strong>{dollars(data.spentMicros)}</strong> spent{budgetMicros > 0 ? ` of the ${dollars(budgetMicros)} monthly budget` : ' (no budget limit)'}.
            {budgetMicros > 0 && share >= 1 ? ' Model calls are paused until next month or a higher budget.' : ''}
          </p>
          {budgetMicros > 0 ? <progress aria-label="Budget used" className="aw-progress__bar" max={1} value={share} /> : null}
        </div>
        {data.usageByModel.length ? (
          <div aria-label="Use by model" className="aw-ai-usage__scroll" role="region" tabIndex={0}>
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
                {data.usageByModel.map((row) => (
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
        ) : (
          <p className="aw-quiet">No model calls this month yet.</p>
        )}
        {data.usageByDay.length ? (
          <ol aria-label="Cost per day, last 30 days" className="aw-ai-days">
            {data.usageByDay.map((day) => (
              <li key={day.day} title={`${day.day}: ${countLabel(day.calls, 'call')}, ${dollars(day.costMicros)}`}>
                <span className="aw-ai-days__bar" style={{ height: `${Math.max(4, (day.costMicros / maxDay) * 100)}%` }} />
                <span className="visually-hidden">
                  {day.day}: {countLabel(day.calls, 'call')}, {dollars(day.costMicros)}
                </span>
              </li>
            ))}
          </ol>
        ) : null}
      </Section>
      <ProviderKeyDialog
        onClose={() => setKeyFor(null)}
        onSaved={() => {
          setKeyFor(null)
          void queryClient.invalidateQueries({ queryKey: aiKeys.workspace(organizationId) })
          void queryClient.invalidateQueries({ queryKey: workspaceKeys.secrets(organizationId) })
        }}
        organizationId={organizationId}
        provider={keyFor}
        replacing={keyFor ? data.workspaceKeys[keyFor.id] === true : false}
      />
    </>
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
    mutationFn: () => putWorkspaceSecret(organizationId, providerKeyName(provider?.id ?? ''), value, `${provider?.displayName ?? ''} API key for models`),
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
      dirty={Boolean(value)}
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
