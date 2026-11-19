import { useQuery } from '@tanstack/react-query'

import { opQuery } from '../../api/ops'
import type { MeterUsage } from '../../api/generated/operations'
import { formatBytes } from '../../files/model'
import { EmptyState, ErrorState, ListSkeleton, PageHeader, StatusChip, formatDateTime } from '../../ui'
import type { SettingsPageProps } from '../types'
import './meters.css'

// H1.25: the workspace's plan and what it has used, meter by meter. Nothing
// here is ever cut: over a limit, only new things that cost are refused.

function amount(meter: MeterUsage, value: number) {
  switch (meter.unit) {
    case 'bytes':
      return formatBytes(value)
    case 'micros':
      return `$${(value / 1_000_000).toFixed(2)}`
    default:
      return value.toLocaleString()
  }
}

const STATUS_WORDS: Record<string, string> = {
  active: 'Active',
  trialing: 'Trial',
  past_due: 'Payment due',
  grace: 'In grace',
  over_limit: 'Over a limit',
}

function MeterRow({ meter }: { meter: MeterUsage }) {
  const limit = meter.limit ?? null
  const share = limit !== null && limit > 0 ? Math.min(1, meter.used / limit) : null
  const near = share !== null && share >= 0.8
  return (
    <li className="aw-meters__meter">
      <div className="aw-meters__line">
        <span className="aw-meters__name">{meter.name.charAt(0).toUpperCase() + meter.name.slice(1)}</span>
        <span className="aw-meters__numbers">
          {amount(meter, meter.used)}
          {limit !== null ? ` of ${amount(meter, limit)}` : ''}
          {meter.kind === 'counter' ? ' this month' : ''}
        </span>
      </div>
      {share !== null && (
        <div
          aria-label={`${Math.round(share * 100)}% of the limit`}
          aria-valuemax={100}
          aria-valuemin={0}
          aria-valuenow={Math.round(share * 100)}
          className={near ? 'aw-meters__bar aw-meters__bar--near' : 'aw-meters__bar'}
          role="progressbar"
        >
          <span style={{ width: `${share * 100}%` }} />
        </div>
      )}
      {meter.resetsAt && limit !== null && <p className="aw-meters__resets">Resets {formatDateTime(meter.resetsAt)}</p>}
    </li>
  )
}

export default function UsagePage({ workspace }: SettingsPageProps) {
  const report = useQuery({ ...opQuery('workspace_usage_get', {}, { workspaceId: workspace?.id }), enabled: Boolean(workspace) })
  if (!workspace) return null
  return (
    <>
      <PageHeader icon="activity" title="Usage">
        What {workspace.name} uses, and its limits. Over a limit, new things that cost are refused; nothing is deleted or hidden.
      </PageHeader>
      {report.isPending ? (
        <ListSkeleton label="Loading usage" rows={6} />
      ) : report.isError ? (
        <ErrorState error={report.error} onRetry={() => void report.refetch()} title="Usage didn’t load" />
      ) : report.data.meters.length === 0 ? (
        <EmptyState icon="activity" title="Nothing counted yet">
          Usage appears here as the workspace is used.
        </EmptyState>
      ) : (
        <section aria-labelledby="usage-plan" className="aw-meters">
          <div className="aw-meters__plan">
            <h2 id="usage-plan">{report.data.planName}</h2>
            <StatusChip tone={report.data.status === 'active' ? 'muted' : 'warning'}>{STATUS_WORDS[report.data.status] ?? report.data.status}</StatusChip>
          </div>
          <ul className="aw-meters__meters">
            {report.data.meters.map((meter) => (
              <MeterRow key={meter.meter} meter={meter} />
            ))}
          </ul>
        </section>
      )}
    </>
  )
}
