import { useState, type ReactNode } from 'react'
import { Link } from 'react-router-dom'

import type { ConversationWorkItemReference } from '../conversations/types'
import { runSettingsFrom } from '../execution/api'
import { Avatar, Button, Icon, IconButton, StatusChip, TextArea, formatDateTime, type ChipTone, type IconName } from '../ui'
import type { Approval, Run, RunState } from './ThreadPage'

const RUN_STATES: Record<RunState, { label: string; tone: ChipTone; icon?: IconName; dots?: boolean; spin?: boolean }> = {
  created: { label: 'Created', tone: 'muted', icon: 'clock' },
  queued: { label: 'Queued', tone: 'muted', icon: 'clock' },
  starting: { label: 'Starting', tone: 'agent', dots: true },
  working: { label: 'Working', tone: 'agent', dots: true },
  blocked: { label: 'Waiting on you', tone: 'warning', icon: 'circle-help' },
  recovering: { label: 'Recovering', tone: 'warning', icon: 'loader', spin: true },
  completed: { label: 'Done', tone: 'success', icon: 'circle-check' },
  failed: { label: 'Failed', tone: 'danger', icon: 'triangle-alert' },
  canceled: { label: 'Stopped', tone: 'muted', icon: 'stop' },
  interrupted: { label: 'Interrupted', tone: 'muted', icon: 'stop' },
}

function shortId(value: string | null | undefined) {
  return value ? value.slice(0, 8) : null
}

function ordinal(position: number) {
  const suffix = position % 10 === 1 && position % 100 !== 11 ? 'st' : position % 10 === 2 && position % 100 !== 12 ? 'nd' : position % 10 === 3 && position % 100 !== 13 ? 'rd' : 'th'
  return `${position}${suffix}`
}

function capitalize(value: string) {
  return value.charAt(0).toLocaleUpperCase() + value.slice(1)
}

function words(value: string) {
  return value.replaceAll('_', ' ')
}

export function RunBadge({ run }: { run: Run | null }) {
  if (!run) return null
  const state = RUN_STATES[run.state]
  return (
    <StatusChip dots={state.dots} icon={state.icon} spin={state.spin} tone={state.tone}>
      Run {state.label.toLocaleLowerCase()}
      {run.state === 'queued' && run.queuePosition ? ` · ${ordinal(run.queuePosition)} in line` : ''}
    </StatusChip>
  )
}

function checksSummary(run: Run) {
  const checks = run.pullRequestChecksState
  if (!checks) return null
  if (checks === 'success' || checks === 'passing' || checks === 'passed') return { icon: 'circle-check' as const, tone: 'success', label: 'Checks passed' }
  if (checks === 'failure' || checks === 'failing' || checks === 'failed' || checks === 'error') return { icon: 'circle-alert' as const, tone: 'danger', label: 'Checks failed' }
  return { icon: 'loader' as const, tone: 'muted', label: `Checks ${words(checks)}` }
}

/** The thread's current run: its state, branch and pull request, and what you can do next. */
export function RunCard({ actions, notes, run }: { actions?: ReactNode; notes?: ReactNode; run: Run }) {
  const state = RUN_STATES[run.state]
  const checks = checksSummary(run)
  const settings = runSettingsFrom(run.config)
  const harness = settings?.harness ? capitalize(words(settings.harness)) : null
  const settingsLine = [settings?.model, settings?.reasoning && `${settings.reasoning} reasoning`, settings?.speed]
    .filter(Boolean)
    .join(' · ')
  return (
    <section aria-label={`Run ${state.label.toLocaleLowerCase()}`} className="aw-card aw-run">
      <header className="aw-card__head">
        <Avatar kind="worker" name="Codex" seed="codex" size={24} />
        <div className="aw-card__titles">
          <span className="aw-card__title">{harness ? `${harness} run` : 'Agent run'}</span>
          <span className="aw-card__sub">Updated {formatDateTime(run.updatedAt)}</span>
        </div>
        <StatusChip dots={state.dots} icon={state.icon} spin={state.spin} tone={state.tone}>
          {state.label}
          {run.state === 'queued' && run.queuePosition ? ` · ${ordinal(run.queuePosition)} in line` : ''}
        </StatusChip>
      </header>
      {settingsLine ? (
        <div className="aw-run__row">
          <Icon name="settings" size={14} />
          <span className="aw-run__meta">{settingsLine}</span>
        </div>
      ) : null}
      {run.branch ? (
        <div className="aw-run__row">
          <Icon name="branch" size={14} />
          <code>{run.branch}</code>
          {run.publishedCommit ? <span className="aw-run__meta">published {run.publishedCommit.slice(0, 7)}</span> : null}
        </div>
      ) : null}
      {run.pullRequestUrl && run.pullRequestNumber ? (
        <div className="aw-run__row">
          <Icon name="pull-request" size={14} />
          <a href={run.pullRequestUrl} rel="noreferrer" target="_blank">
            #{run.pullRequestNumber} {run.pullRequestTitle}
          </a>
          {run.pullRequestDraft ? <span className="aw-run__meta">draft</span> : null}
          {checks ? (
            <span className={`aw-run__checks is-${checks.tone}`}>
              <Icon className={checks.icon === 'loader' ? 'aw-spin' : undefined} name={checks.icon} size={14} />
              {checks.label}
            </span>
          ) : null}
        </div>
      ) : null}
      {run.pullRequestUrl && (run.pullRequestReviewState || run.pullRequestMergeabilityState) ? (
        <p className="aw-run__detail">
          {run.pullRequestState ? `${words(run.pullRequestState)} · ` : ''}
          review {words(run.pullRequestReviewState ?? 'unknown')} · {words(run.pullRequestMergeabilityState ?? 'mergeability unknown')}
        </p>
      ) : null}
      {run.failureReason ? (
        <p className="aw-run__error">
          <Icon name="circle-alert" size={14} />
          {run.failureReason}
        </p>
      ) : null}
      {notes}
      {actions ? <footer className="aw-card__actions">{actions}</footer> : null}
    </section>
  )
}

/** A decision the run needs from you: allow or deny, or answer a question. */
export function ApprovalCard({
  approval,
  busyAction,
  canRespond,
  onRespond,
}: {
  approval: Approval
  busyAction: string | null
  canRespond: boolean
  onRespond: (approvalId: string, response: { decision: 'approve' | 'deny' } | { answer: string }) => void
}) {
  const [answer, setAnswer] = useState('')
  const question = approval.kind === 'question'
  return (
    <section aria-labelledby={`approval-${approval.id}`} className="aw-card aw-approval is-open">
      <header className="aw-card__head">
        <Avatar kind="worker" name="Codex" seed="codex" size={24} />
        <div className="aw-card__titles">
          <h2 className="aw-card__title" id={`approval-${approval.id}`}>
            {question ? 'Codex has a question' : 'Codex needs your approval'}
          </h2>
        </div>
        <StatusChip icon={question ? 'circle-help' : 'shield-check'} tone="warning">
          Needs you
        </StatusChip>
      </header>
      <p className="aw-approval__summary">{approval.summary}</p>
      {canRespond && question ? (
        <form
          className="aw-approval__answer-form"
          onSubmit={(event) => {
            event.preventDefault()
            const value = answer.trim()
            if (value) onRespond(approval.id, { answer: value })
          }}
        >
          {approval.options.length > 0 ? (
            <div aria-label="Suggested answers" className="aw-card__actions" role="group">
              {approval.options.map((option) => (
                <Button disabled={Boolean(busyAction)} key={option.id} onClick={() => setAnswer(option.label)} size="sm">
                  {option.label}
                </Button>
              ))}
            </div>
          ) : null}
          <TextArea
            disabled={Boolean(busyAction)}
            id={`approval-answer-${approval.id}`}
            label="Your answer"
            maxLength={4000}
            onChange={(event) => setAnswer(event.target.value)}
            required
            rows={3}
            value={answer}
          />
          <div className="aw-card__actions">
            <Button busy={busyAction === `${approval.id}:answer`} disabled={Boolean(busyAction) || !answer.trim()} type="submit" variant="primary">
              Send answer
            </Button>
          </div>
        </form>
      ) : null}
      {canRespond && !question ? (
        <footer className="aw-card__actions">
          <Button
            busy={busyAction === `${approval.id}:approve`}
            disabled={Boolean(busyAction)}
            onClick={() => onRespond(approval.id, { decision: 'approve' })}
            variant="primary"
          >
            Allow once
          </Button>
          <Button
            busy={busyAction === `${approval.id}:deny`}
            disabled={Boolean(busyAction)}
            onClick={() => onRespond(approval.id, { decision: 'deny' })}
          >
            Deny
          </Button>
        </footer>
      ) : null}
      {!canRespond ? <p className="aw-approval__note">Only people who can run work in this Project can answer.</p> : null}
    </section>
  )
}

/** The details panel: the run's workspace and the work items this thread is about. */
export function ThreadDetails({
  onClose,
  onUnlink,
  projectName,
  run,
  threadId,
  unlinking,
  workItems,
}: {
  onClose?: () => void
  onUnlink?: (workItemId: string) => void
  projectName: string
  run: Run | null
  threadId: string
  unlinking: boolean
  workItems: ConversationWorkItemReference[]
}) {
  const rows: [string, ReactNode][] = [
    ['Project', projectName],
    ['Runs on', 'Codex, in an isolated workspace'],
    ['Branch', run?.branch ? <code>{run.branch}</code> : 'Assigned when a run starts'],
    ['Base commit', shortId(run?.baseCommit) ? <code>{shortId(run?.baseCommit)}</code> : '—'],
    ['Run', shortId(run?.id) ? <code>{shortId(run?.id)}</code> : 'No run yet'],
    ['Thread', <code key="thread">{shortId(threadId)}</code>],
  ]
  return (
    <aside aria-label="Thread details" className="context-drawer aw-details">
      <div className="aw-details__head">
        <h2>Details</h2>
        {onClose ? <IconButton icon="x" label="Close details" onClick={onClose} size="sm" /> : null}
      </div>
      {run ? (
        <div className="aw-details__state">
          <RunBadge run={run} />
        </div>
      ) : null}
      <dl className="aw-details__list">
        {rows.map(([label, value]) => (
          <div key={label}>
            <dt>{label}</dt>
            <dd>{value}</dd>
          </div>
        ))}
      </dl>
      <section className="aw-details__section">
        <h3>Linked work items</h3>
        {workItems.length ? (
          <ul className="aw-details__items">
            {workItems.map((item) => (
              <li key={item.id}>
                <Link className="aw-workref" to={item.href}>
                  <span className="aw-workref__key">{item.key}</span>
                  <span className="aw-workref__title">{item.title}</span>
                </Link>
                {item.archivedAt ? <span className="aw-details__archived">Archived</span> : null}
                {onUnlink ? (
                  <Button disabled={unlinking} onClick={() => onUnlink(item.id)} size="sm" variant="ghost">
                    Unlink
                  </Button>
                ) : null}
              </li>
            ))}
          </ul>
        ) : (
          <p className="aw-details__empty">No linked work items. Link one from its page.</p>
        )}
      </section>
    </aside>
  )
}
