import { useQuery } from '@tanstack/react-query'
import { useState, type ReactNode } from 'react'
import { Link, useParams } from 'react-router-dom'

import { isApiError } from '../api/errors'
import { useAuthenticatedSession } from '../auth/session'
import { threadHref } from '../launch/launchModel'
import { AppShell } from '../shell/AppShell'
import {
  Banner,
  Button,
  DetailList,
  EmptyState,
  ErrorState,
  PageHeader,
  PageSkeleton,
  Section,
  SegmentedControl,
  StatusChip,
  formatDateTime,
} from '../ui'
import { workItemHref } from '../work-items/links'
import { getRun, runKeys, type RunDetail } from './api'
import { RunDiff } from './RunDiff'
import { RunSteps } from './RunSteps'
import { formatCost, formatTokens, isLiveRun, logText, runStatus, runTitle } from './runModel'
import './runs.css'

const LIVE_REFRESH_MS = 5_000

/** One run: what it was asked, every step it took, what it changed and what it used. */
export function RunPage() {
  const session = useAuthenticatedSession()
  const { runId = '' } = useParams()
  const run = useQuery({
    queryKey: runKeys.detail(runId),
    queryFn: () => getRun(runId),
    enabled: Boolean(runId),
    refetchInterval: (query) => (query.state.data && isLiveRun(query.state.data.state) ? LIVE_REFRESH_MS : false),
  })
  const notFound = run.isError && isApiError(run.error) && run.error.status === 404

  return (
    <AppShell mainClassName="aw-main--page" user={session.user}>
      <div className="aw-page aw-runs">
        <Link className="aw-runs__back" to="/runs">
          All runs
        </Link>
        {run.isPending ? <PageSkeleton /> : null}
        {notFound ? (
          <EmptyState action={<Link to="/runs">See all runs</Link>} icon="activity" title="This run isn’t here">
            It may be in a thread you can’t see.
          </EmptyState>
        ) : null}
        {run.isError && !notFound ? (
          <ErrorState action={<Button onClick={() => void run.refetch()}>Try again</Button>} title="The run didn’t load">
            {run.error.message}
          </ErrorState>
        ) : null}
        {run.data ? <RunBody run={run.data} /> : null}
      </div>
    </AppShell>
  )
}

function RunBody({ run }: { run: RunDetail }) {
  const status = runStatus(run.state)
  const [view, setView] = useState<'steps' | 'log'>('steps')
  const details: { label: string; value: ReactNode }[] = [
    { label: 'Project', value: run.projectName },
    { label: 'Agent', value: run.agentName ?? 'Not named' },
    { label: 'Harness', value: run.harnessName ?? run.harness ?? 'Not known' },
    { label: 'Model', value: [run.model, run.reasoning, run.speed].filter(Boolean).join(' · ') || 'Harness default' },
  ]
  if (run.taskId) {
    details.push({ label: 'Task', value: <Link to={workItemHref(run.projectId, run.taskId)}>{runTitle({ ...run, threadName: null })}</Link> })
  }
  if (run.conversationId) {
    details.push({ label: 'Thread', value: <Link to={threadHref(run.projectId, run.conversationId)}>{run.threadName ?? 'Open the thread'}</Link> })
  }
  if (run.branch) details.push({ label: 'Branch', value: <code>{run.branch}</code> })
  if (run.pullRequestUrl) {
    details.push({
      label: 'Pull request',
      value: (
        <a href={run.pullRequestUrl} rel="noreferrer" target="_blank">
          {run.pullRequestNumber ? `#${run.pullRequestNumber}` : 'Open'}
          {run.pullRequestState ? ` (${run.pullRequestState})` : ''}
        </a>
      ),
    })
  }
  details.push({ label: 'Started by', value: run.requestedByName ?? 'Not known' })
  details.push({ label: 'Created', value: formatDateTime(run.createdAt) })
  if (run.finishedAt) details.push({ label: 'Finished', value: formatDateTime(run.finishedAt) })

  return (
    <>
      <PageHeader
        actions={<StatusChip icon={status.icon} spin={status.spin} tone={status.tone}>{status.label}</StatusChip>}
        icon="activity"
        title={runTitle(run)}
      />
      {run.failureReason ? (
        <Banner tone="danger" title="Why it stopped:">
          {run.failureReason}
        </Banner>
      ) : null}
      <DetailList items={details} />
      <Section
        actions={
          <SegmentedControl
            label="Show as"
            onChange={setView}
            options={[
              { value: 'steps', label: 'Steps' },
              { value: 'log', label: 'Log' },
            ]}
            value={view}
          />
        }
        id="run-steps"
        title="What it did"
      >
        {view === 'steps' ? <RunSteps hidden={run.hiddenSteps} steps={run.steps} /> : <pre className="aw-runs__pre aw-runs__log">{logText(run.steps) || 'No steps yet.'}</pre>}
      </Section>
      <Section id="run-files" title="Files changed">
        <RunDiff files={run.files} runId={run.id} />
      </Section>
      <Section id="run-tests" title="Tests">
        <TestsSummary run={run} />
      </Section>
      <Section id="run-usage" title="Tokens and cost">
        <ModelUsageTable run={run} />
      </Section>
      {run.prompt ? (
        <Section id="run-prompt" title="What it was asked">
          <pre className="aw-runs__pre aw-runs__prompt">{run.prompt}</pre>
        </Section>
      ) : null}
    </>
  )
}

function TestsSummary({ run }: { run: RunDetail }) {
  const { tests } = run
  if (tests.runs === 0) return <p className="aw-runs__quiet">The run didn’t report any tests.</p>
  const last =
    tests.lastFailed === null ? null : tests.lastFailed === 0 ? 'The last test run passed.' : `The last test run had ${tests.lastFailed} failing.`
  return (
    <p className="aw-runs__tests">
      <StatusChip tone={tests.lastFailed ? 'danger' : 'success'}>{tests.lastFailed ? 'Failing' : 'Passing'}</StatusChip>{' '}
      {tests.runs} test {tests.runs === 1 ? 'run' : 'runs'}: {tests.passed} passed, {tests.failed} failed, {tests.skipped} skipped. {last}
    </p>
  )
}

function ModelUsageTable({ run }: { run: RunDetail }) {
  if (run.usageByModel.length === 0) return <p className="aw-runs__quiet">The run hasn’t reported any tokens.</p>
  return (
    <div className="aw-runs__tablewrap">
      <table className="aw-runs__table">
        <caption className="aw-sr-only">Tokens and cost per model</caption>
        <thead>
          <tr>
            <th scope="col">Model</th>
            <th scope="col">Input</th>
            <th scope="col">Cached</th>
            <th scope="col">Output</th>
            <th scope="col">Cost</th>
          </tr>
        </thead>
        <tbody>
          {run.usageByModel.map((row) => (
            <tr key={row.model}>
              <th scope="row">{row.model}</th>
              <td>{formatTokens(row.inputTokens)}</td>
              <td>{formatTokens(row.cachedInputTokens)}</td>
              <td>{formatTokens(row.outputTokens)}</td>
              <td>{formatCost(row.cost, row.currency) ?? 'Not known'}</td>
            </tr>
          ))}
        </tbody>
        {run.usageByModel.length > 1 ? (
          <tfoot>
            <tr>
              <th scope="row">Total</th>
              <td>{formatTokens(run.inputTokens)}</td>
              <td>{formatTokens(run.cachedInputTokens)}</td>
              <td>{formatTokens(run.outputTokens)}</td>
              <td>{formatCost(run.cost, run.currency) ?? 'Not known'}</td>
            </tr>
          </tfoot>
        ) : null}
      </table>
    </div>
  )
}
