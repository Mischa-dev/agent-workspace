import { useState, type FormEvent } from 'react'
import { useQuery } from '@tanstack/react-query'

import { toastError } from '../app/mutations'
import { useWorkspaceId } from '../app/workspace'
import { opQuery, useOp } from '../api/ops'
import { Button, Row, RowList, SelectField, TextField } from '../ui'
import '../styles/pages.css'

type Period = 'minute' | 'hour' | 'day' | 'month'

const PERIOD_WORDS: Record<string, string> = { minute: 'a minute', hour: 'an hour', day: 'a day', month: 'a month' }

/** A number in its meter's unit: micros are millionths of a dollar. */
function amount(value: number, unit: string): string {
  return unit === 'micros' ? `$${(value / 1_000_000).toFixed(2)}` : value.toLocaleString()
}

/**
 * A2.14: the budgets on one agent, for whoever manages the workspace's
 * settings: what applies to it (on it, and on each account on its own),
 * its use so far, and a form to set one. Others see nothing here.
 */
export function AgentBudgets({ accountId, name }: { accountId: string; name: string }) {
  const workspaceId = useWorkspaceId() ?? undefined
  const budgets = useQuery({ ...opQuery('budget_list', { account: accountId }, { workspaceId }), retry: false })
  const set = useOp('budget_set', { workspaceId, invalidates: ['budget_list'] })
  const [meter, setMeter] = useState('operations')
  const [period, setPeriod] = useState<Period>('day')
  const [hard, setHard] = useState('')
  const headingId = `budgets-heading-${accountId}`

  if (!budgets.data) return null
  const units = new Map(budgets.data.meters.map((entry) => [entry.id, entry.unit]))

  function submit(event: FormEvent) {
    event.preventDefault()
    const limit = Number.parseInt(hard, 10)
    if (!Number.isFinite(limit) || limit < 0) return
    const scaled = units.get(meter) === 'micros' ? limit * 1_000_000 : limit
    set.mutate(
      { scope: 'account', scopeId: accountId, meter, period, hardLimit: scaled },
      {
        onSuccess: () => setHard(''),
        onError: (reason) => toastError(`The budget for ${name} wasn’t set`, reason),
      },
    )
  }

  return (
    <section aria-labelledby={headingId} className="aw-webhook">
      <h3 className="aw-row__title" id={headingId}>
        Budgets
      </h3>
      <p className="aw-quiet">Past a budget, more is refused, the same as for a person. Pushes wait for the next period.</p>
      {budgets.data.budgets.length ? (
        <RowList label={`Budgets for ${name}`}>
          {budgets.data.budgets.map((budget) => {
            const limit = budget.hardLimit ?? budget.softLimit ?? 0
            const own = budget.scope === 'account' && budget.scopeId === accountId
            return (
              <Row
                actions={
                  own ? (
                    <Button
                      disabled={set.isPending}
                      onClick={() =>
                        set.mutate(
                          { scope: 'account', scopeId: accountId, meter: budget.meter, period: budget.period as Period },
                          { onError: (reason) => toastError(`The budget for ${name} wasn’t removed`, reason) },
                        )
                      }
                      size="sm"
                      variant="ghost"
                    >
                      Remove
                    </Button>
                  ) : null
                }
                key={budget.id ?? `${budget.meter}-${budget.period}`}
                meta={`${amount(budget.used, budget.unit)} used`}
                title={`${budget.meterName}: ${amount(limit, budget.unit)} ${PERIOD_WORDS[budget.period] ?? budget.period}`}
              >
                {budget.isDefault ? 'The workspace’s default for everyone' : own ? null : budget.eachAccount ? 'For each account' : 'Shared'}
              </Row>
            )
          })}
        </RowList>
      ) : null}
      <form className="aw-inline-form" onSubmit={submit}>
        <SelectField id={`budget-meter-${accountId}`} label="Counts" onChange={(event) => setMeter(event.target.value)} value={meter}>
          {budgets.data.meters.map((entry) => (
            <option key={entry.id} value={entry.id}>
              {entry.name}
            </option>
          ))}
        </SelectField>
        <SelectField
          id={`budget-period-${accountId}`}
          label="Per"
          onChange={(event) => setPeriod(event.target.value as Period)}
          value={period}
        >
          <option value="minute">Minute</option>
          <option value="hour">Hour</option>
          <option value="day">Day</option>
          <option value="month">Month</option>
        </SelectField>
        <TextField
          id={`budget-limit-${accountId}`}
          inputMode="numeric"
          label={units.get(meter) === 'micros' ? 'Limit in dollars' : 'Limit'}
          onChange={(event) => setHard(event.target.value.replace(/[^0-9]/g, ''))}
          value={hard}
        />
        <Button busy={set.isPending} disabled={!hard} size="sm" type="submit">
          Set budget
        </Button>
      </form>
    </section>
  )
}
