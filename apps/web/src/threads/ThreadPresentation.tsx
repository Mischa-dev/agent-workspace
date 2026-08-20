import { useState } from 'react'
import { Link } from 'react-router-dom'

import { MessageContent } from '../conversations/MessageContent'
import { SystemTimelineEntry } from '../conversations/SystemTimelineEntry'
import type {
  ConversationTimelineEntry,
  ConversationWorkItemReference,
} from '../conversations/types'
import type { Approval, Run, RunState } from './ThreadPage'

const stateLabels: Record<RunState, string> = {
  created: 'Created',
  queued: 'Queued',
  starting: 'Starting',
  working: 'Working',
  blocked: 'Needs input',
  recovering: 'Recovering',
  completed: 'Completed',
  failed: 'Failed',
  canceled: 'Canceled',
  interrupted: 'Interrupted',
}

function shortId(value: string | null | undefined) {
  return value ? `${value.slice(0, 8)}…` : '—'
}

export function RunBadge({ run }: { run: Run | null }) {
  if (!run) {
    return <span className="status-chip" data-state="settled">Ready</span>
  }
  return (
    <span className="status-chip" data-state={run.state}>
      <span aria-hidden="true">{run.state === 'blocked' ? '!' : '●'}</span>
      {stateLabels[run.state]}
      {run.state === 'queued' && run.queuePosition
        ? ` · ${run.queuePosition} in queue`
        : null}
    </span>
  )
}

export function TimelineItem({ entry }: { entry: ConversationTimelineEntry }) {
  if (entry.message) {
    const worker = entry.message.author.kind === 'execution_worker'
    const avatar = worker
      ? 'W'
      : entry.message.author.kind === 'external_agent'
        ? 'E'
        : 'H'
    return (
      <article className={`message ${worker ? 'worker-message' : ''}`}>
        <header>
          <span className="avatar" data-identity={entry.message.author.kind} aria-hidden="true">{avatar}</span>
          <strong>@{entry.message.author.handle}</strong>
          <span className="identity-kind">{entry.message.author.kind.replaceAll('_', ' ')}</span>
          <time dateTime={entry.createdAt}>
            {new Date(entry.createdAt).toLocaleTimeString([], {
              hour: 'numeric',
              minute: '2-digit',
            })}
          </time>
        </header>
        <p>
          <MessageContent
            actors={entry.message.mentionedActors ?? []}
            content={entry.message.content}
            fallbackPlainText={entry.message.plainText}
            workItems={entry.message.referencedWorkItems ?? []}
          />
        </p>
      </article>
    )
  }
  if (entry.workerEvent) {
    return (
      <article className="provider-event" data-kind={entry.workerEvent.kind}>
        <span className="event-icon" aria-hidden="true">{eventIcon(entry.workerEvent.kind)}</span>
        <div>
          <strong>{eventLabel(entry.workerEvent.kind)}</strong>
          <p>{entry.workerEvent.summary}</p>
        </div>
        <time dateTime={entry.createdAt}>
          {new Date(entry.createdAt).toLocaleTimeString([], {
            hour: 'numeric',
            minute: '2-digit',
          })}
        </time>
      </article>
    )
  }
  return <SystemTimelineEntry createdAt={entry.createdAt} />
}

function eventIcon(kind: string) {
  if (kind.includes('file')) return 'Δ'
  if (kind.includes('approval') || kind.includes('question')) return '?'
  if (kind.includes('interrupt') || kind.includes('failed')) return '×'
  if (kind.includes('completed')) return '✓'
  if (kind.includes('recovery')) return '↻'
  return '·'
}

function eventLabel(kind: string) {
  return kind
    .split('_')
    .map((part) => `${part.charAt(0).toUpperCase()}${part.slice(1)}`)
    .join(' ')
}

export function ApprovalPrompt({
  approval,
  busyAction,
  canRespond,
  onRespond,
}: {
  approval: Approval
  busyAction: string | null
  canRespond: boolean
  onRespond: (
    approvalId: string,
    response: { decision: 'approve' | 'deny' } | { answer: string },
  ) => void
}) {
  const [answer, setAnswer] = useState('')
  const answerAction = `${approval.id}:answer`
  return (
    <section aria-labelledby={`approval-${approval.id}`} className="approval-prompt">
      <div className="approval-symbol" aria-hidden="true">?</div>
      <div>
        <h2 id={`approval-${approval.id}`}>
          {approval.kind === 'question' ? 'Worker question' : 'Approval required'}
        </h2>
        <p>{approval.summary}</p>
        {canRespond && approval.kind === 'question' && (
          <form className="approval-actions" onSubmit={(event) => { event.preventDefault(); const value = answer.trim(); if (value) onRespond(approval.id, { answer: value }) }}>
            <label htmlFor={`approval-answer-${approval.id}`}>Your answer</label>
            {approval.options.length > 0 && (
              <div aria-label="Suggested answers" className="approval-actions">
                {approval.options.map((option) => (
                  <button className="secondary-button" disabled={Boolean(busyAction)} key={option.id} onClick={() => setAnswer(option.label)} type="button">{option.label}</button>
                ))}
              </div>
            )}
            <textarea disabled={Boolean(busyAction)} id={`approval-answer-${approval.id}`} maxLength={4000} onChange={(event) => setAnswer(event.target.value)} required rows={3} value={answer} />
            <button className="primary-button" disabled={Boolean(busyAction) || !answer.trim()} type="submit">
              {busyAction === answerAction ? 'Sending…' : 'Send answer'}
            </button>
          </form>
        )}
        {canRespond && approval.kind === 'approval' && (
          <div className="approval-actions">
            <button className="primary-button" disabled={Boolean(busyAction)} onClick={() => onRespond(approval.id, { decision: 'approve' })} type="button">
              {busyAction === `${approval.id}:approve` ? 'Allowing…' : 'Allow once'}
            </button>
            <button className="secondary-button" disabled={Boolean(busyAction)} onClick={() => onRespond(approval.id, { decision: 'deny' })} type="button">
              {busyAction === `${approval.id}:deny` ? 'Denying…' : 'Deny'}
            </button>
          </div>
        )}
      </div>
    </section>
  )
}

export function TimelineSkeleton() {
  return <div aria-label="Loading Thread" className="timeline-skeleton"><span /><span /><span /></div>
}

export function ThreadContext({
  projectName,
  run,
  threadId,
  workItems,
  onUnlink,
  unlinking,
}: {
  projectName: string
  run: Run | null
  threadId: string
  workItems: ConversationWorkItemReference[]
  onUnlink?: (workItemId: string) => void
  unlinking: boolean
}) {
  return (
    <aside aria-label="Execution context" className="context-drawer">
      <span className="section-label">Execution context</span>
      <h2>Project host</h2>
      <dl>
        <div><dt>Project</dt><dd>{projectName}</dd></div>
        <div><dt>Provider</dt><dd>Codex App Server</dd></div>
        <div><dt>Thread</dt><dd>{shortId(threadId)}</dd></div>
        <div><dt>Run</dt><dd>{shortId(run?.id)}</dd></div>
        <div><dt>Workspace</dt><dd>{shortId(run?.repositoryWorkspaceId)}</dd></div>
        <div><dt>Branch</dt><dd>{run?.branch ?? 'Allocated after admission'}</dd></div>
        <div><dt>Base commit</dt><dd>{shortId(run?.baseCommit)}</dd></div>
      </dl>
      <div className="thread-linked-work-items">
        <h3>Linked Work Items</h3>
        {workItems.length ? (
          <ul>
            {workItems.map((item) => (
              <li key={item.id}>
                <div><Link to={item.href}>{item.key}</Link><span>{item.title}</span>{item.archivedAt ? <small>Archived</small> : null}</div>
                {onUnlink ? <button className="text-button" disabled={unlinking} onClick={() => onUnlink(item.id)} type="button">Unlink</button> : null}
              </li>
            ))}
          </ul>
        ) : <p className="compact-note">No linked Work Items.</p>}
      </div>
      <p className="context-note">Authentication and authorship are resolved from the server-side session.</p>
    </aside>
  )
}
