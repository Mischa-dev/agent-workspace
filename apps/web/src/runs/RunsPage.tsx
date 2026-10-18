import { useInfiniteQuery } from '@tanstack/react-query'
import { Link, useSearchParams } from 'react-router-dom'

import { useAuthenticatedSession } from '../auth/session'
import { AppShell } from '../shell/AppShell'
import { Button, EmptyState, ErrorState, ListSkeleton, PageHeader, Row, RowList, SegmentedControl, StatusChip, Switch, formatRelative } from '../ui'
import { listRuns, runKeys, type RunListFilter, type RunStatusFilter, type RunSummary } from './api'
import { RunUsage } from './RunUsage'
import { formatCost, formatTokens, isLiveRun, runStatus, runTitle, totalTokens } from './runModel'
import './runs.css'

const TABS: { value: RunStatusFilter; label: string }[] = [
  { value: 'all', label: 'All' },
  { value: 'active', label: 'Active' },
  { value: 'needs_you', label: 'Needs you' },
  { value: 'finished', label: 'Finished' },
  { value: 'failed', label: 'Failed' },
]

const EMPTY: Record<RunStatusFilter, string> = {
  all: 'Runs agents do on your tasks show up here.',
  active: 'Nothing is running right now.',
  needs_you: 'No run is waiting for you.',
  finished: 'No run has finished yet.',
  failed: 'No run has failed.',
}

/** Runs refresh on their own while any of them is still going. */
const LIVE_REFRESH_MS = 15_000

function statusParam(value: string | null): RunStatusFilter {
  return TABS.some((tab) => tab.value === value) ? (value as RunStatusFilter) : 'all'
}

/** Every agent run you can see, across your projects, newest first. */
export function RunsPage() {
  const session = useAuthenticatedSession()
  const [params, setParams] = useSearchParams()
  const status = statusParam(params.get('status'))
  const mine = params.get('mine') === '1'
  const projectId = params.get('project') ?? undefined
  const filter: RunListFilter = { status, mine, projectId }

  const runs = useInfiniteQuery({
    queryKey: runKeys.list(filter),
    queryFn: ({ pageParam }) => listRuns(filter, pageParam),
    initialPageParam: undefined as string | undefined,
    getNextPageParam: (page) => page.nextCursor ?? undefined,
    refetchInterval: (query) =>
      query.state.data?.pages.some((page) => page.runs.some((run) => isLiveRun(run.state))) ? LIVE_REFRESH_MS : false,
  })
  const list = runs.data?.pages.flatMap((page) => page.runs) ?? []

  const update = (key: string, value: string | null) => {
    const next = new URLSearchParams(params)
    if (value) next.set(key, value)
    else next.delete(key)
    setParams(next, { replace: true })
  }

  return (
    <AppShell home mainClassName="aw-main--page" user={session.user}>
      <div className="aw-page aw-runs">
        <PageHeader icon="activity" title="Runs">
          What agents did on your tasks, what they used and what it cost.
        </PageHeader>
        <div className="aw-runs__filters">
          <SegmentedControl label="Show runs" onChange={(value) => update('status', value === 'all' ? null : value)} options={TABS} value={status} />
          <Switch checked={mine} label="Only runs I started" onChange={(checked) => update('mine', checked ? '1' : null)} />
        </div>
        {runs.isPending ? <ListSkeleton label="Loading runs" rows={6} /> : null}
        {runs.isError ? (
          <ErrorState action={<Button onClick={() => void runs.refetch()}>Try again</Button>} title="Runs didn’t load">
            {runs.error.message}
          </ErrorState>
        ) : null}
        {runs.data && list.length === 0 ? (
          <EmptyState icon="activity" title="No runs here">
            {EMPTY[status]}
          </EmptyState>
        ) : null}
        {list.length > 0 ? (
          <RowList label="Runs">
            {list.map((run) => (
              <RunRow key={run.id} run={run} />
            ))}
          </RowList>
        ) : null}
        {runs.hasNextPage ? (
          <div className="aw-runs__more">
            <Button busy={runs.isFetchingNextPage} onClick={() => void runs.fetchNextPage()}>
              Show more runs
            </Button>
          </div>
        ) : null}
        <RunUsage projectId={projectId} />
      </div>
    </AppShell>
  )
}

function RunRow({ run }: { run: RunSummary }) {
  const status = runStatus(run.state)
  const cost = formatCost(run.cost, run.currency)
  const tokens = totalTokens(run)
  return (
    <Row
      actions={<StatusChip icon={status.icon} spin={status.spin} tone={status.tone}>{status.label}</StatusChip>}
      className="aw-runs__row"
      meta={
        <>
          <span>{run.projectName}</span>
          {run.harnessName ? <span>{run.harnessName}</span> : null}
          {run.agentName ? <span>{run.agentName}</span> : null}
          <span>{tokens > 0 ? `${formatTokens(tokens)} tokens` : 'No tokens yet'}</span>
          {cost ? <span>{cost}</span> : null}
          <span>{formatRelative(run.createdAt)}</span>
        </>
      }
      title={
        <Link className="aw-runs__link" to={`/runs/${run.id}`}>
          {runTitle(run)}
        </Link>
      }
    />
  )
}
