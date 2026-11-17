import { useState, type ReactNode } from 'react'
import { Link } from 'react-router-dom'

import type { ConversationWorkItemChip } from '../conversations/types'
import { runSettingsFrom } from '../execution/api'
import { Avatar, Button, Icon, IconButton, StatusChip, TextArea, formatFullDate, formatRelative, type ChipTone, type IconName } from '../ui'
import { RUN_STATUS } from '../runs/runModel'
import { runAgent } from './runAgent'
import { answerKey } from './threadTimeline'
import type { RequestAnswer, Run, ThreadRequest } from './threadTypes'
import '../styles/conversation.css'

function ordinal(position: number) {
  const suffix = position % 10 === 1 && position % 100 !== 11 ? 'st' : position % 10 === 2 && position % 100 !== 12 ? 'nd' : position % 10 === 3 && position % 100 !== 13 ? 'rd' : 'th'
  return `${position}${suffix}`
}

function words(value: string) {
  return value.replaceAll('_', ' ')
}

export function RunBadge({ run }: { run: Run | null }) {
  if (!run) return null
  const state = RUN_STATUS[run.state]
  return (
    <StatusChip icon={state.icon} spin={state.spin} tone={state.tone}>
      Run {state.label.toLocaleLowerCase()}
      {run.state === 'queued' && run.queuePosition ? ` · ${ordinal(run.queuePosition)} in line` : ''}
    </StatusChip>
  )
}

type Chip = { label: string; tone: ChipTone; icon?: IconName }

/** A pull request's state, review and whether it can merge, as chips. */
function pullRequestChips(run: Run): Chip[] {
  const chips: Chip[] = []
  const state = run.pullRequestState
  if (state === 'merged') chips.push({ label: 'Merged', tone: 'success', icon: 'circle-check' })
  else if (state === 'closed') chips.push({ label: 'Closed', tone: 'muted' })
  else if (run.pullRequestDraft) chips.push({ label: 'Draft', tone: 'muted' })
  else if (state === 'open') chips.push({ label: 'Open', tone: 'agent' })
  if (state !== 'open') return chips
  const review = run.pullRequestReviewState
  if (review === 'approved') chips.push({ label: 'Approved', tone: 'success', icon: 'check' })
  else if (review === 'changes_requested') chips.push({ label: 'Changes requested', tone: 'warning', icon: 'circle-alert' })
  else if (review === 'pending') chips.push({ label: 'Waiting for review', tone: 'muted', icon: 'clock' })
  if (run.pullRequestMergeabilityState === 'ready') chips.push({ label: 'Ready to merge', tone: 'success' })
  return chips
}

function checksSummary(run: Run) {
  const checks = run.pullRequestChecksState
  if (!checks) return null
  if (checks === 'success' || checks === 'passing' || checks === 'passed') return { icon: 'circle-check' as const, tone: 'success', label: 'Checks passed' }
  if (checks === 'failure' || checks === 'failing' || checks === 'failed' || checks === 'error') return { icon: 'circle-alert' as const, tone: 'danger', label: 'Checks failed' }
  return { icon: 'loader' as const, tone: 'muted', label: `Checks ${words(checks)}` }
}

/**
 * The thread's current run: its state, branch and pull request, and what you
 * can do next. `developerDetail` is off when the project's layout hides branch
 * names and commit hashes.
 */
export function RunCard({ actions, developerDetail = true, notes, run }: { actions?: ReactNode; developerDetail?: boolean; notes?: ReactNode; run: Run }) {
  const state = RUN_STATUS[run.state]
  const checks = checksSummary(run)
  const settings = runSettingsFrom(run.config)
  const agent = runAgent(run)
  const settingsLine = [settings?.model, settings?.reasoning && `${settings.reasoning} reasoning`, settings?.speed]
    .filter(Boolean)
    .join(' · ')
  return (
    <section aria-label={`Run ${state.label.toLocaleLowerCase()}`} className="aw-card aw-run">
      <header className="aw-card__head">
        <Avatar kind="agent" name={agent.name} seed={agent.seed} size={24} />
        <div className="aw-card__titles">
          <span className="aw-card__title">{settings?.harness ? `Run by ${agent.name}` : 'Agent run'}</span>
          <span className="aw-card__sub" title={formatFullDate(run.updatedAt)}>
            Updated {formatRelative(run.updatedAt)}
          </span>
        </div>
        <StatusChip icon={state.icon} spin={state.spin} tone={state.tone}>
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
      {run.branch && developerDetail ? (
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
          {checks ? (
            <span className={`aw-run__checks is-${checks.tone}`}>
              <Icon className={checks.icon === 'loader' ? 'aw-spin' : undefined} name={checks.icon} size={14} />
              {checks.label}
            </span>
          ) : null}
        </div>
      ) : null}
      {run.pullRequestUrl && pullRequestChips(run).length ? (
        <div aria-label="Pull request" className="aw-run__chips" role="group">
          {pullRequestChips(run).map((chip) => (
            <StatusChip icon={chip.icon} key={chip.label} tone={chip.tone}>
              {chip.label}
            </StatusChip>
          ))}
        </div>
      ) : null}
      {run.failureReason ? (
        <p className="aw-run__error">
          <Icon name="circle-alert" size={14} />
          {run.failureReason}
        </p>
      ) : null}
      <div className="aw-run__row">
        <Icon name="activity" size={14} />
        <Link to={`/runs/${encodeURIComponent(run.id)}`}>See every step, the diff and the cost</Link>
      </div>
      {notes}
      {actions ? <footer className="aw-card__actions">{actions}</footer> : null}
    </section>
  )
}

/** A request the run raised: allow or deny, or answer a question. */
export function ApprovalCard({
  approval,
  busyAction,
  canRespond,
  onRespond,
  run,
}: {
  approval: ThreadRequest
  /** The run asking, for its name. */
  run: Run | null
  busyAction: string | null
  canRespond: boolean
  onRespond: (requestId: string, answer: RequestAnswer) => void
}) {
  const [answer, setAnswer] = useState('')
  const question = approval.kind !== 'approval'
  const agent = runAgent(run)
  const send = (reply: RequestAnswer) => onRespond(approval.id, reply)
  return (
    <section aria-labelledby={`approval-${approval.id}`} className="aw-card aw-approval is-open">
      <header className="aw-card__head">
        <Avatar kind="agent" name={agent.name} seed={agent.seed} size={24} />
        <div className="aw-card__titles">
          <h2 className="aw-card__title" id={`approval-${approval.id}`}>
            {question ? `${agent.name} has a question` : `${agent.name} needs your approval`}
          </h2>
        </div>
        <StatusChip icon={question ? 'circle-help' : 'shield-check'} tone="warning">
          Needs you
        </StatusChip>
      </header>
      <p className="aw-approval__summary">{approval.title}</p>
      {canRespond ? (
        <form
          className="aw-approval__answer-form"
          onSubmit={(event) => {
            event.preventDefault()
            const value = answer.trim()
            if (value) send({ text: value })
          }}
        >
          {approval.options.length > 0 ? (
            <div
              aria-label={question ? 'Suggested answers' : 'Answers'}
              className="aw-card__actions"
              role="group"
            >
              {/* An offered answer is an answer: it sends straight away. */}
              {approval.options.map((option) => (
                <Button
                  busy={busyAction === answerKey(approval.id, { optionId: option.id })}
                  disabled={Boolean(busyAction)}
                  key={option.id}
                  onClick={() => send({ optionId: option.id })}
                  size={question ? 'sm' : undefined}
                  type="button"
                  variant={(option.style ?? option.id) === 'approve' ? 'primary' : undefined}
                >
                  {option.label}
                </Button>
              ))}
            </div>
          ) : null}
          {approval.allowText ? (
            <>
              <TextArea
                disabled={Boolean(busyAction)}
                id={`approval-answer-${approval.id}`}
                label={approval.options.length > 0 ? 'Or write your own' : 'Your answer'}
                maxLength={4000}
                onChange={(event) => setAnswer(event.target.value)}
                required
                rows={3}
                value={answer}
              />
              <div className="aw-card__actions">
                <Button
                  busy={busyAction === answerKey(approval.id, {})}
                  disabled={Boolean(busyAction) || !answer.trim()}
                  type="submit"
                  variant="primary"
                >
                  Send answer
                </Button>
              </div>
            </>
          ) : null}
        </form>
      ) : null}
      {!canRespond ? <p className="aw-approval__note">This waits on whoever it asks; they answer it here or from their inbox.</p> : null}
    </section>
  )
}

/** The details panel: the run's workspace and the tasks this thread is about. */
export function ThreadDetails({
  onClose,
  onUnlink,
  projectName,
  run,
  unlinking,
  workItems,
}: {
  onClose?: () => void
  onUnlink?: (workItemId: string) => void
  projectName: string
  run: Run | null
  unlinking: boolean
  workItems: ConversationWorkItemChip[]
}) {
  const rows: [string, ReactNode][] = [
    ['Project', projectName],
    ['Runs on', run ? `${runAgent(run).name}, in an isolated workspace` : 'An isolated workspace'],
    ['Branch', run?.branch ? <code>{run.branch}</code> : 'Assigned when a run starts'],
    ['Latest run', run ? <Link to={`/runs/${encodeURIComponent(run.id)}`}>View run</Link> : 'No run yet'],
  ]
  return (
    <aside aria-label="Thread details" className="context-drawer aw-details">
      <div className="aw-details__head">
        <h2>Details</h2>
        {onClose ? <IconButton className="aw-panelclose" icon="x" label="Close details" onClick={onClose} size="sm" /> : null}
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
        <h3>Linked tasks</h3>
        {workItems.length ? (
          <ul className="aw-details__items">
            {workItems.map((item) => (
              <li key={item.id}>
                <Link className="aw-workref" to={item.href}>
                  <span className="aw-workref__key">{item.key}</span>
                  {item.title ? <span className="aw-workref__title">{item.title}</span> : null}
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
          <p className="aw-details__empty">No linked tasks. Link one from the task’s page.</p>
        )}
      </section>
    </aside>
  )
}
