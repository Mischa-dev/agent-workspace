import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useEffect, useRef, useState, type ReactNode } from 'react'
import { Link, useParams } from 'react-router-dom'

import { isApiError } from '../api/errors'
import { queryKeys } from '../api/queryKeys'
import { toastError } from '../app/mutations'
import { useAuthenticatedSession } from '../auth/session'
import { useProjectExperience } from '../experience/layout'
import { threadHref } from '../launch/launchModel'
import { AppShell } from '../shell/AppShell'
import { usePageTitle } from '../shell/documentTitleModel'
import { ApprovalCard } from '../threads/ThreadPresentation'
import { RunCardActions } from '../threads/ThreadRunControls'
import { useThread } from '../threads/threadTimeline'
import { useThreadRunActions } from '../threads/useThreadRunActions'
import {
  Banner,
  Button,
  ButtonLink,
  DetailList,
  EmptyState,
  ErrorState,
  Icon,
  PageHeader,
  PageSkeleton,
  Section,
  SegmentedControl,
  StatusChip,
  countLabel,
  eventLabel,
  formatDateTime,
  formatFullDate,
  showToast,
} from '../ui'
import { workItemHref } from '../work-items/links'
import { getRun, runKeys, type RunDetail } from './api'
import { RunDiff } from './RunDiff'
import { RunSteps } from './RunSteps'
import { formatCost, formatTokens, isLiveRun, logText, runDuration, runStatus, runTitle } from './runModel'
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
  usePageTitle(run.data ? runTitle(run.data) : null)

  return (
    <AppShell home mainClassName="aw-main--page" user={session.user}>
      <div className="aw-page aw-runs">
        <ButtonLink className="aw-runs__back" icon="arrow-left" size="sm" to="/runs" variant="ghost">
          All runs
        </ButtonLink>
        {run.isPending ? <PageSkeleton /> : null}
        {notFound ? (
          <EmptyState action={<ButtonLink to="/runs">See all runs</ButtonLink>} icon="activity" title="This run isn’t here">
            It may be in a thread you can’t see.
          </EmptyState>
        ) : null}
        {run.isError && !notFound ? <ErrorState error={run.error} onRetry={() => void run.refetch()} title="The run didn’t load" /> : null}
        {run.data ? <RunBody run={run.data} /> : null}
      </div>
    </AppShell>
  )
}

/** Ticks while the run is live, so its running time keeps counting. */
function useNow(live: boolean) {
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    if (!live) return
    const timer = window.setInterval(() => setNow(Date.now()), 15_000)
    return () => window.clearInterval(timer)
  }, [live])
  return now
}

const STOP_TITLE: Partial<Record<RunDetail['state'], string>> = {
  failed: 'The run failed.',
  interrupted: 'The run was interrupted.',
  canceled: 'The run was stopped.',
}

function RunBody({ run }: { run: RunDetail }) {
  const status = runStatus(run.state)
  const live = isLiveRun(run.state)
  const now = useNow(live)
  const [view, setView] = useState<'steps' | 'log'>('steps')
  const queryClient = useQueryClient()

  // The thread holds the run's controls (stop, answer, run again, publish), so
  // the page borrows them; they only show while this run is the thread's
  // current one, the same rule the thread's own run card follows.
  const threadId = run.conversationId ?? ''
  const thread = useThread(run.projectId, threadId, false)
  const experience = useProjectExperience(run.projectId)
  const usage = experience.shows('tech.run_usage')
  const timeline = thread.routeMatches ? thread.timeline.data : undefined
  const actions = useThreadRunActions({ entries: timeline?.entries ?? [], includeArchived: false, projectId: run.projectId, threadId, timeline })
  const activeRun = timeline?.activeRun ?? null
  const latestRun = timeline?.latestTerminalRun ?? null
  const current = activeRun ? activeRun.id === run.id : latestRun?.id === run.id
  const later = (other: { createdAt: string }) => new Date(other.createdAt).getTime() > new Date(run.createdAt).getTime()
  const newer = activeRun && activeRun.id !== run.id ? activeRun : !activeRun && latestRun && latestRun.id !== run.id && later(latestRun) ? latestRun : null
  const approval = timeline?.approvals.find((candidate) => candidate.status === 'pending' && candidate.agentRunId === run.id) ?? null

  // When the run moves on, the thread's view of it does too.
  const signature = `${run.state}:${run.updatedAt}`
  const seen = useRef(signature)
  useEffect(() => {
    if (seen.current === signature || !threadId) return
    seen.current = signature
    void queryClient.invalidateQueries({ queryKey: queryKeys.timeline(run.projectId, threadId, false) })
  }, [queryClient, run.projectId, signature, threadId])
  // And when an action finishes, the run page reads the run again.
  const wasBusy = useRef(false)
  useEffect(() => {
    if (actions.busyAction) {
      wasBusy.current = true
      return
    }
    if (!wasBusy.current) return
    wasBusy.current = false
    void queryClient.invalidateQueries({ queryKey: runKeys.detail(run.id) })
    void queryClient.invalidateQueries({ queryKey: ['runs', 'list'] })
  }, [actions.busyAction, queryClient, run.id])

  const copyLog = () => {
    navigator.clipboard.writeText(logText(run.steps)).then(
      () => showToast({ title: 'Log copied' }),
      (error: unknown) => toastError('The log wasn’t copied', error),
    )
  }

  const duration = runDuration(run, now)
  const files = run.files?.length ?? 0
  const cost = usage ? formatCost(run.cost, run.currency) : null
  const summary = [
    run.agentName,
    duration ? (live ? `${duration} so far` : duration) : null,
    files ? `${countLabel(files, 'file')} changed` : null,
    run.tests.runs > 0 && run.tests.lastFailed !== null ? (run.tests.lastFailed > 0 ? 'tests failing' : 'tests passing') : null,
    cost,
  ].filter(Boolean)

  const details: { label: string; value: ReactNode }[] = [{ label: 'Project', value: run.projectName }]
  if (run.agentName) details.push({ label: 'Agent', value: run.agentName })
  if (run.harnessName ?? run.harness) details.push({ label: 'Harness', value: run.harnessName ?? run.harness })
  details.push({ label: 'Model', value: [run.model, run.reasoning, run.speed].filter(Boolean).join(' · ') || 'Harness default' })
  if (run.taskId) {
    details.push({ label: 'Task', value: <Link to={workItemHref(run.projectId, run.taskId)}>{runTitle({ ...run, threadName: null })}</Link> })
  }
  if (run.conversationId) {
    details.push({ label: 'Thread', value: <Link to={threadHref(run.projectId, run.conversationId)}>{run.threadName ?? 'Open the thread'}</Link> })
  }
  if (run.branch && experience.shows('tech.ids')) details.push({ label: 'Branch', value: <code>{run.branch}</code> })
  if (run.pullRequestUrl) {
    details.push({
      label: 'Pull request',
      value: (
        <a href={run.pullRequestUrl} rel="noreferrer" target="_blank">
          {[run.pullRequestNumber ? `#${run.pullRequestNumber}` : 'Open on GitHub', run.pullRequestState ? eventLabel(run.pullRequestState) : null]
            .filter(Boolean)
            .join(' · ')}
        </a>
      ),
    })
  }
  if (run.requestedByName) details.push({ label: 'Requested by', value: run.requestedByName })
  details.push({ label: 'Requested', value: <time dateTime={run.createdAt} title={formatFullDate(run.createdAt)}>{formatDateTime(run.createdAt)}</time> })
  if (run.finishedAt) {
    details.push({ label: 'Finished', value: <time dateTime={run.finishedAt} title={formatFullDate(run.finishedAt)}>{formatDateTime(run.finishedAt)}</time> })
  }

  return (
    <>
      <PageHeader
        actions={
          <div className="aw-runs__actions">
            <StatusChip icon={status.icon} spin={status.spin} tone={status.tone}>
              {status.label}
            </StatusChip>
            {timeline && current ? <RunCardActions actions={actions} activeRun={activeRun} latestTerminalRun={latestRun} /> : null}
            {run.pullRequestUrl ? (
              <a className="aw-btn aw-btn--secondary aw-btn--sm" href={run.pullRequestUrl} rel="noreferrer" target="_blank">
                <Icon name="pull-request" size={14} />
                <span className="aw-btn__label">Open pull request</span>
              </a>
            ) : null}
          </div>
        }
        icon="activity"
        title={runTitle(run)}
      >
        {summary.length ? summary.join(' · ') : null}
      </PageHeader>
      {newer ? (
        <Banner
          action={
            <ButtonLink size="sm" to={`/runs/${encodeURIComponent(newer.id)}`}>
              Open it
            </ButtonLink>
          }
          tone="info"
        >
          {isLiveRun(newer.state as RunDetail['state']) ? 'A newer run is going in this thread.' : 'This thread has a newer run.'}
        </Banner>
      ) : null}
      {approval ? (
        <ApprovalCard
          approval={approval}
          busyAction={actions.busyAction}
          canRespond={approval.capabilities.respond}
          onRespond={actions.respond}
          run={[activeRun, latestRun].find((candidate) => candidate?.id === run.id) ?? activeRun}
        />
      ) : run.state === 'blocked' && run.conversationId ? (
        <Banner
          action={
            <ButtonLink size="sm" to={threadHref(run.projectId, run.conversationId)}>
              Answer in the thread
            </ButtonLink>
          }
          tone="warning"
        >
          {run.agentName ? `${run.agentName} is waiting on your answer.` : 'This run is waiting on your answer.'}
        </Banner>
      ) : null}
      {run.failureReason ? (
        <Banner tone={STOP_TITLE[run.state] ? 'danger' : 'warning'} title={STOP_TITLE[run.state] ?? 'The run hit a problem.'}>
          {run.failureReason}
        </Banner>
      ) : null}
      <div className="aw-runs__details">
        <DetailList items={details} />
      </div>
      <Section id="run-files" title="Files changed">
        <RunDiff live={live} run={run} />
      </Section>
      <Section id="run-tests" title="Tests">
        <TestsSummary run={run} />
      </Section>
      <Section
        actions={
          <div className="aw-runs__actions">
            {view === 'log' && run.steps.length > 0 ? (
              <Button icon="copy" onClick={copyLog} size="sm" variant="ghost">
                Copy log
              </Button>
            ) : null}
            <SegmentedControl
              label="Show as"
              onChange={setView}
              options={[
                { value: 'steps', label: 'Steps' },
                { value: 'log', label: 'Log' },
              ]}
              value={view}
            />
          </div>
        }
        id="run-steps"
        title="What it did"
      >
        {view === 'steps' ? (
          <RunSteps hidden={run.hiddenSteps} steps={run.steps} />
        ) : (
          <pre className="aw-runs__pre aw-runs__log">{logText(run.steps) || 'No steps yet.'}</pre>
        )}
      </Section>
      {run.prompt ? (
        <Section id="run-prompt" title="What it was asked">
          <pre className="aw-runs__pre aw-runs__prompt">{run.prompt}</pre>
        </Section>
      ) : null}
      {usage ? (
        <Section id="run-usage" title="Tokens and cost">
          <ModelUsageTable live={live} run={run} />
        </Section>
      ) : null}
    </>
  )
}

function TestsSummary({ run }: { run: RunDetail }) {
  const { tests } = run
  if (tests.runs === 0) return <p className="aw-runs__quiet">The run didn’t report any tests.</p>
  const last =
    tests.lastFailed === null
      ? null
      : tests.lastFailed === 0
        ? 'The last run passed.'
        : `${countLabel(tests.lastFailed, 'test')} failed in the last run.`
  return (
    <p className="aw-runs__tests">
      {tests.lastFailed === null ? null : (
        <>
          <StatusChip tone={tests.lastFailed ? 'danger' : 'success'}>{tests.lastFailed ? 'Failing' : 'Passing'}</StatusChip>{' '}
        </>
      )}
      {last ? `${last} ` : null}
      Across {countLabel(tests.runs, 'test run')}: {tests.passed} passed, {tests.failed} failed, {tests.skipped} skipped.
    </p>
  )
}

function ModelUsageTable({ live, run }: { live: boolean; run: RunDetail }) {
  if (run.usageByModel.length === 0) {
    return <p className="aw-runs__quiet">{live ? 'The run hasn’t reported any tokens yet.' : 'The run didn’t report any tokens.'}</p>
  }
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
