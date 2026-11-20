import { useQuery } from '@tanstack/react-query'
import { useState, type FormEvent } from 'react'

import { opQuery, useOp } from '../../api/ops'
import type { BudgetPeriod, BudgetScope, BudgetView } from '../../api/generated/operations'
import { Banner, Button, EmptyState, describeError, ErrorState, ListSkeleton, Row, RowList, Section, SelectField, TextField } from '../../ui'

// H1.27, A2.14: limits the workspace sets on itself (its budgets), per
// minute, hour, day or month, on the whole workspace, a space, a
// conversation or one account. Over a soft limit whoever set it is told;
// over a hard one the next ask is refused.

const PERIOD_WORDS: Record<string, string> = { minute: 'a minute', hour: 'an hour', day: 'a day', month: 'a month' }

const SCOPE_WORDS: Record<string, string> = {
  workspace: 'The workspace',
  space: 'Space',
  conversation: 'Conversation',
  account: 'Account',
}

function whose(limit: BudgetView) {
  const scope = limit.scope === 'workspace' ? SCOPE_WORDS.workspace : `One ${(SCOPE_WORDS[limit.scope] ?? limit.scope).toLowerCase()}`
  const who = limit.eachAccount ? `Each account in ${scope.charAt(0).toLowerCase()}${scope.slice(1)}` : scope
  return limit.isDefault ? `${who} (default)` : who
}

function describe(limit: BudgetView) {
  const period = PERIOD_WORDS[limit.period] ?? limit.period
  const parts = []
  if (limit.softLimit != null) parts.push(`tell at ${limit.softLimit.toLocaleString()}`)
  if (limit.hardLimit != null) parts.push(`stop at ${limit.hardLimit.toLocaleString()}`)
  const used = ` · ${limit.used.toLocaleString()} used`
  return `${limit.meterName}, ${period}: ${parts.join(', ')}${used}`
}

type Who = 'workspace' | 'each'

export function SpendLimits({ workspaceId }: { workspaceId: string }) {
  const listed = useQuery(opQuery('budget_list', {}, { workspaceId }))
  const write = useOp('budget_set', { workspaceId, invalidates: ['budget_list'] })
  const [who, setWho] = useState<Who>('workspace')
  const [meter, setMeter] = useState('')
  const [period, setPeriod] = useState<BudgetPeriod>('day')
  const [soft, setSoft] = useState('')
  const [hard, setHard] = useState('')

  // Each account's limit counts a minute or an hour.
  const chosenPeriod: BudgetPeriod = who === 'each' && period !== 'minute' && period !== 'hour' ? 'minute' : period
  const chosenMeter = meter || listed.data?.meters[0]?.id || ''

  function add(event: FormEvent) {
    event.preventDefault()
    const softLimit = soft.trim() === '' ? undefined : Number(soft)
    const hardLimit = hard.trim() === '' ? undefined : Number(hard)
    write.mutate(
      {
        scope: 'workspace',
        meter: chosenMeter,
        period: chosenPeriod,
        eachAccount: who === 'each' ? true : undefined,
        softLimit,
        hardLimit,
      },
      {
        onSuccess: () => {
          setSoft('')
          setHard('')
        },
      },
    )
  }

  // Both limits left out removes it.
  function remove(limit: BudgetView) {
    write.mutate({
      scope: limit.scope as BudgetScope,
      scopeId: limit.scope === 'workspace' ? undefined : (limit.scopeId ?? undefined),
      meter: limit.meter,
      period: limit.period as BudgetPeriod,
      eachAccount: limit.eachAccount || undefined,
      expectedVersion: limit.version ?? undefined,
    })
  }

  return (
    <Section
      description="Over a soft limit, whoever set it is told in their inbox. Over a hard one, the next message or write is refused until the period turns over."
      id="usage-limits"
      title="Limits"
    >
      {listed.isPending ? (
        <ListSkeleton label="Loading limits" rows={2} />
      ) : listed.isError ? (
        <ErrorState error={listed.error} onRetry={() => void listed.refetch()} title="Limits didn’t load" />
      ) : (
        <>
          {listed.data.budgets.length === 0 ? (
            <EmptyState icon="activity" title="No limits set">
              Nothing is held back beyond the plan.
            </EmptyState>
          ) : (
            <RowList label="Limits">
              {listed.data.budgets.map((limit) => (
                <Row
                  actions={
                    limit.isDefault ? undefined : (
                      <Button disabled={write.isPending} onClick={() => remove(limit)} size="sm" variant="ghost">
                        Remove
                      </Button>
                    )
                  }
                  key={limit.id ?? `${limit.scope}:${limit.meter}:${limit.period}`}
                  meta={describe(limit)}
                  title={whose(limit)}
                />
              ))}
            </RowList>
          )}
          <form className="aw-meters__limitform" onSubmit={add}>
            <SelectField compact label="Who" onChange={(event) => setWho(event.target.value as Who)} value={who}>
              <option value="workspace">The whole workspace</option>
              <option value="each">Each account</option>
            </SelectField>
            <SelectField compact label="What" onChange={(event) => setMeter(event.target.value)} value={chosenMeter}>
              {listed.data.meters.map((option) => (
                <option key={option.id} value={option.id}>
                  {option.name.charAt(0).toUpperCase() + option.name.slice(1)}
                </option>
              ))}
            </SelectField>
            <SelectField
              compact
              label="Per"
              onChange={(event) => setPeriod(event.target.value as BudgetPeriod)}
              value={chosenPeriod}
            >
              <option value="minute">Minute</option>
              <option value="hour">Hour</option>
              {who === 'workspace' && <option value="day">Day</option>}
              {who === 'workspace' && <option value="month">Month</option>}
            </SelectField>
            <TextField compact inputMode="numeric" label="Tell at" min={1} onChange={(event) => setSoft(event.target.value)} type="number" value={soft} />
            <TextField compact inputMode="numeric" label="Stop at" min={1} onChange={(event) => setHard(event.target.value)} type="number" value={hard} />
            <Button busy={write.isPending} disabled={!chosenMeter || (hard === '' && soft === '')} type="submit" variant="primary">
              Set limit
            </Button>
          </form>
          {write.isError && <Banner tone="danger">The limit wasn’t saved. {describeError(write.error).message}</Banner>}
          <p className="aw-meters__resets">Limits on one space, conversation or account can be set by anyone who manages settings, from an app or agent.</p>
        </>
      )}
    </Section>
  )
}
