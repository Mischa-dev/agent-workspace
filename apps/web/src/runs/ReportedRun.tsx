import { useMutation, useQueryClient } from '@tanstack/react-query'
import { useState, type ReactNode } from 'react'
import { Link } from 'react-router-dom'

import { toastError } from '../app/mutations'
import { threadHref } from '../launch/launchModel'
import { Banner, Button, DetailList, Icon, PageHeader, Section, StatusChip, formatDateTime, formatFullDate, formatTime, showToast } from '../ui'
import { workItemHref } from '../work-items/links'
import { cancelRun, listRunActivities, markRunNotStarted, runKeys, type RunActivity, type RunView } from './api'
import {
  activityView,
  causeLabel,
  costText,
  formatTokens,
  hostLabel,
  isLiveRun,
  livenessLabel,
  planMark,
  runDuration,
  runStatus,
  runTitle,
} from './runModel'

const CHECKS: Record<string, string> = { success: 'checks passed', failure: 'checks failing', pending: 'checks running' }
const REVIEW: Record<string, string> = { approved: 'approved', changes_requested: 'changes requested', pending: 'waiting for review' }

/** "Open · checks passing · approved"; states nobody knows yet are left out. */
function pullRequestLine(pullRequest: RunView['pullRequests'][number]) {
  const state = pullRequest.draft ? 'Draft' : pullRequest.state.charAt(0).toUpperCase() + pullRequest.state.slice(1)
  return [state, CHECKS[pullRequest.checksState], REVIEW[pullRequest.reviewState]].filter(Boolean).join(' · ')
}

const STOP_TITLE: Partial<Record<RunView['state'], string>> = {
  failed: 'The run failed.',
  refused: 'The host refused the run.',
  interrupted: 'The run was interrupted.',
  cancelled: 'The run was stopped.',
}

/**
 * A run that a host or the agent itself reports (A1.5): its state and
 * whether it is still reporting, what it waits on, its plan, what it did,
 * the requests it raised, what it produced and what it used.
 */
export function ReportedRun({ now, onRefresh, run, usage }: { now: number; onRefresh: () => void; run: RunView; usage: boolean }) {
  const queryClient = useQueryClient()
  const status = runStatus(run.state)
  const live = isLiveRun(run.state)
  const quiet = livenessLabel(run, now)
  const refresh = () => {
    void queryClient.invalidateQueries({ queryKey: runKeys.view(run.id) })
    void queryClient.invalidateQueries({ queryKey: runKeys.lists })
  }
  const stop = useMutation({
    mutationFn: () => cancelRun(run.id),
    onSuccess: () => {
      showToast({ title: 'Asked the run to stop' })
      refresh()
    },
    onError: (error) => toastError('The run wasn’t stopped', error),
  })
  const notStarted = useMutation({
    mutationFn: () => markRunNotStarted(run.id),
    onSuccess: refresh,
    onError: (error) => toastError('The run wasn’t marked', error),
  })

  const duration = runDuration(run, now)
  const cost = usage ? costText(run.cost, run.currency, run.costConfidence) : null
  const summary = [run.agentName, hostLabel(run), duration ? (live ? `${duration} so far` : duration) : null, cost?.text].filter(Boolean)

  const details: { label: string; value: ReactNode }[] = []
  if (run.projectName) details.push({ label: 'Project', value: run.projectName })
  details.push({ label: 'Agent', value: run.agentName })
  details.push({ label: 'Runs on', value: hostLabel(run) })
  details.push({ label: 'Why', value: causeLabel(run) })
  if (run.taskId && run.projectId) {
    details.push({ label: 'Task', value: <Link to={workItemHref(run.projectId, run.taskId)}>{runTitle({ ...run, threadName: null })}</Link> })
  }
  if (run.conversationId && run.projectId) {
    details.push({ label: 'Thread', value: <Link to={threadHref(run.projectId, run.conversationId)}>{run.threadName ?? 'Open the thread'}</Link> })
  }
  if (run.requestedByName) details.push({ label: 'Requested by', value: run.requestedByName })
  details.push({ label: 'Requested', value: <time dateTime={run.createdAt} title={formatFullDate(run.createdAt)}>{formatDateTime(run.createdAt)}</time> })
  if (run.finishedAt) {
    details.push({ label: 'Finished', value: <time dateTime={run.finishedAt} title={formatFullDate(run.finishedAt)}>{formatDateTime(run.finishedAt)}</time> })
  }

  const stoppable = live && run.state !== 'cancel_requested'
  return (
    <>
      <PageHeader
        actions={
          <div className="aw-runs__actions">
            {quiet ? <StatusChip tone={quiet.tone}>{quiet.label}</StatusChip> : null}
            <StatusChip icon={status.icon} spin={status.spin} tone={status.tone}>
              {status.label}
            </StatusChip>
            {stoppable ? (
              <Button busy={stop.isPending} icon="stop" onClick={() => stop.mutate()} size="sm">
                Stop
              </Button>
            ) : null}
            {run.externalUrl ? (
              <a className="aw-btn aw-btn--secondary aw-btn--sm" href={run.externalUrl} rel="noreferrer" target="_blank">
                <Icon name="external" size={14} />
                <span className="aw-btn__label">Open session</span>
              </a>
            ) : null}
          </div>
        }
        icon="activity"
        title={runTitle(run)}
      >
        {summary.join(' · ')}
      </PageHeader>
      {run.state === 'start_uncertain' ? (
        <Banner
          action={
            <div className="aw-runs__actions">
              <Button icon="retry" onClick={onRefresh} size="sm">
                Check again
              </Button>
              <Button busy={notStarted.isPending} onClick={() => notStarted.mutate()} size="sm">
                Mark as not started
              </Button>
            </div>
          }
          title="The host didn’t say whether the run started."
          tone="warning"
        >
          If it never started, mark it so it can be started again.
        </Banner>
      ) : null}
      {run.waitingOnTitle ? (
        <Banner tone="warning" title="Waiting on an answer:">
          {run.waitingOnTitle}
        </Banner>
      ) : null}
      {run.failureReason ? (
        <Banner tone={STOP_TITLE[run.state] ? 'danger' : 'warning'} title={STOP_TITLE[run.state] ?? 'The run hit a problem.'}>
          {run.failureReason}
        </Banner>
      ) : null}
      {run.gapCount > 0 ? (
        <Banner tone="info">
          {run.gapCount === 1 ? 'One stretch of this run wasn’t reported' : `${run.gapCount} stretches of this run weren’t reported`}, so its timeline has gaps.
        </Banner>
      ) : null}
      {live && run.statusLine ? <p className="aw-runs__status">{run.statusLine}</p> : null}
      {!live && run.outcomeSummary ? <p className="aw-runs__status">{run.outcomeSummary}</p> : null}
      <div className="aw-runs__details">
        <DetailList items={details} />
      </div>
      {run.plan?.length ? (
        <Section id="run-plan" title="Plan">
          <ol className="aw-runs__plan">
            {run.plan.map((item, index) => {
              const mark = planMark(item.status)
              return (
                <li className={`aw-runs__planitem aw-runs__planitem--${item.status ?? 'pending'}`} key={`${index}-${item.title}`}>
                  <Icon label={mark.label} name={mark.icon} size={14} />
                  <span>{item.title}</span>
                </li>
              )
            })}
          </ol>
        </Section>
      ) : null}
      <Section id="run-activity" title="What it did">
        <Timeline run={run} />
      </Section>
      {run.requests.length ? (
        <Section id="run-requests" title="Requests">
          <ul className="aw-runs__requests">
            {run.requests.map((request) => (
              <li key={request.id}>
                <StatusChip tone={request.status === 'open' ? 'warning' : 'muted'}>{request.status === 'open' ? 'Open' : request.status === 'answered' ? 'Answered' : 'Closed'}</StatusChip>{' '}
                {request.conversationId && run.projectId ? (
                  <Link to={threadHref(run.projectId, request.conversationId)}>{request.title}</Link>
                ) : (
                  request.title
                )}
              </li>
            ))}
          </ul>
        </Section>
      ) : null}
      {run.pullRequests.length ? (
        <Section id="run-pull-requests" title="Pull requests">
          <ul className="aw-runs__results">
            {run.pullRequests.map((pullRequest) => (
              <li key={pullRequest.id}>
                <a href={pullRequest.url} rel="noreferrer" target="_blank">
                  {pullRequest.repository}#{pullRequest.number} {pullRequest.title}
                </a>{' '}
                <span className="aw-runs__quiet">{pullRequestLine(pullRequest)}</span>
              </li>
            ))}
          </ul>
        </Section>
      ) : null}
      {run.results.length ? (
        <Section id="run-results" title="What it produced">
          <ul className="aw-runs__results">
            {run.results.map((result, index) => {
              const label = result.title ?? (result.number ? `#${result.number}` : (result.branch ?? result.id ?? result.url ?? result.kind))
              return (
                <li key={`${result.kind}-${index}`}>
                  <span className="aw-runs__resultkind">{result.kind.replaceAll('_', ' ')}</span>{' '}
                  {result.url ? (
                    <a href={result.url} rel="noreferrer" target="_blank">
                      {label}
                    </a>
                  ) : (
                    <code>{label}</code>
                  )}
                </li>
              )
            })}
          </ul>
        </Section>
      ) : null}
      {usage ? (
        <Section id="run-usage" title="Tokens and cost">
          <UsageTable live={live} run={run} />
        </Section>
      ) : null}
    </>
  )
}

/** The run's activities, oldest first; earlier ones load on request. */
function Timeline({ run }: { run: RunView }) {
  const [earlier, setEarlier] = useState<RunActivity[]>([])
  const [loading, setLoading] = useState(false)
  const firstShown = run.activities[0]?.seq ?? Number.MAX_SAFE_INTEGER
  const hidden = Math.max(0, run.hiddenActivities - earlier.length)
  const loadEarlier = async () => {
    setLoading(true)
    try {
      const page = await listRunActivities(run.id, earlier.at(-1)?.seq ?? 0)
      setEarlier((loaded) => [...loaded, ...page.activities.filter((activity) => activity.seq < firstShown)])
    } catch (error) {
      toastError('Earlier activities didn’t load', error)
    } finally {
      setLoading(false)
    }
  }
  const activities = [...earlier, ...run.activities]
  if (activities.length === 0) return <p className="aw-runs__quiet">The run hasn’t reported anything yet.</p>
  return (
    <>
      {hidden > 0 ? (
        <p className="aw-runs__quiet">
          {hidden === 1 ? 'One earlier activity isn’t shown.' : `${hidden.toLocaleString()} earlier activities aren’t shown.`}{' '}
          <Button busy={loading} onClick={() => void loadEarlier()} size="sm" variant="ghost">
            Show earlier
          </Button>
        </p>
      ) : null}
      <ol className="aw-runsteps">
        {activities.map((activity) => {
          const view = activityView(activity)
          return (
            <li className={`aw-runsteps__item aw-runsteps__item--${view.tone}`} key={activity.seq}>
              <span aria-hidden className="aw-runsteps__icon">
                <Icon name={view.icon} size={14} />
              </span>
              <div className="aw-runsteps__body">
                <div className="aw-runsteps__head">
                  <span className="aw-runsteps__title">{view.title}</span>
                  <time className="aw-runsteps__time" dateTime={activity.occurredAt}>
                    {formatTime(activity.occurredAt)}
                  </time>
                </div>
              </div>
            </li>
          )
        })}
      </ol>
    </>
  )
}

function UsageTable({ live, run }: { live: boolean; run: RunView }) {
  if (run.usageByModel.length === 0) {
    return <p className="aw-runs__quiet">{live ? 'The run hasn’t reported any tokens yet.' : 'The run didn’t report any tokens.'}</p>
  }
  const total = costText(run.cost, run.currency, run.costConfidence)
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
          {run.usageByModel.map((row) => {
            const cost = costText(row.cost, row.currency, row.confidence)
            return (
              <tr key={row.model}>
                <th scope="row">{row.model}</th>
                <td>{formatTokens(row.inputTokens)}</td>
                <td>{formatTokens(row.cachedInputTokens)}</td>
                <td>{formatTokens(row.outputTokens)}</td>
                <td title={cost?.hint ?? undefined}>{cost?.text ?? 'Not known'}</td>
              </tr>
            )
          })}
        </tbody>
        {run.usageByModel.length > 1 ? (
          <tfoot>
            <tr>
              <th scope="row">Total</th>
              <td>{formatTokens(run.inputTokens)}</td>
              <td>{formatTokens(run.cachedInputTokens)}</td>
              <td>{formatTokens(run.outputTokens)}</td>
              <td title={total?.hint ?? undefined}>{total?.text ?? 'Not known'}</td>
            </tr>
          </tfoot>
        ) : null}
      </table>
      {run.costConfidence === 'estimated' ? (
        <p className="aw-runs__quiet">≈ marks a cost shared out from a usage window the host reported, not one reported for this run.</p>
      ) : null}
    </div>
  )
}
