import { useQuery } from '@tanstack/react-query'
import { useState } from 'react'

import { Button, ErrorState, ListSkeleton, Section, SegmentedControl, formatShortDate } from '../ui'
import { getUsage, runKeys, type UsageGroup, type UsageReport, type UsageRow } from './api'
import { formatCost, formatTokens } from './runModel'

const GROUPS: { value: UsageGroup; label: string }[] = [
  { value: 'day', label: 'Day' },
  { value: 'project', label: 'Project' },
  { value: 'account', label: 'Account' },
  { value: 'model', label: 'Model' },
]

const COLUMN: Record<UsageGroup, string> = { day: 'Day', project: 'Project', account: 'Account', model: 'Model' }

const DAYS = 30

/** Tokens and cost of the runs you can see over the last month, totalled the way you pick. */
export function RunUsage({ organizationId, projectId }: { organizationId?: string; projectId?: string }) {
  const [groupBy, setGroupBy] = useState<UsageGroup>('day')
  const filter = { organizationId, projectId, groupBy, days: DAYS }
  const usage = useQuery({ queryKey: runKeys.usage(filter), queryFn: () => getUsage(filter) })

  return (
    <Section
      description={`What runs used in the last ${DAYS} days. Cost shows where the price of the model is known.`}
      id="runs-usage"
      title="Tokens and cost"
    >
      <SegmentedControl label="Group by" onChange={setGroupBy} options={GROUPS} value={groupBy} />
      {usage.isPending ? <ListSkeleton label="Loading tokens and cost" rows={4} /> : null}
      {usage.isError ? (
        <ErrorState action={<Button onClick={() => void usage.refetch()}>Try again</Button>} title="Tokens and cost didn’t load">
          {usage.error.message}
        </ErrorState>
      ) : null}
      {usage.data ? <UsageTable groupBy={groupBy} report={usage.data} /> : null}
    </Section>
  )
}

function rowLabel(groupBy: UsageGroup, row: UsageRow) {
  if (groupBy === 'day') return formatShortDate(`${row.key}T12:00:00`)
  return row.label || row.key
}

function UsageTable({ groupBy, report }: { groupBy: UsageGroup; report: UsageReport }) {
  if (report.rows.length === 0) {
    return <p className="aw-runs__quiet">No runs used tokens since {formatShortDate(`${report.since}T12:00:00`)}.</p>
  }
  return (
    <div className="aw-runs__tablewrap">
      <table className="aw-runs__table">
        <caption className="aw-sr-only">Tokens and cost by {COLUMN[groupBy].toLowerCase()}</caption>
        <thead>
          <tr>
            <th scope="col">{COLUMN[groupBy]}</th>
            <th scope="col">Runs</th>
            <th scope="col">Input</th>
            <th scope="col">Output</th>
            <th scope="col">Cost</th>
          </tr>
        </thead>
        <tbody>
          {report.rows.map((row) => (
            <tr key={row.key}>
              <th scope="row">{rowLabel(groupBy, row)}</th>
              <td>{row.runs}</td>
              <td>{formatTokens(row.inputTokens)}</td>
              <td>{formatTokens(row.outputTokens)}</td>
              <td>{formatCost(row.cost, row.currency) ?? 'Not known'}</td>
            </tr>
          ))}
        </tbody>
        <tfoot>
          <tr>
            <th scope="row">Total</th>
            <td>{report.runs}</td>
            <td>{formatTokens(report.totals.inputTokens)}</td>
            <td>{formatTokens(report.totals.outputTokens)}</td>
            <td>{formatCost(report.totals.cost, report.totals.currency) ?? 'Not known'}</td>
          </tr>
        </tfoot>
      </table>
    </div>
  )
}
