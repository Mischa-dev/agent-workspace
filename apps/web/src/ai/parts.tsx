import { Link } from 'react-router-dom'

import { Banner, Button, ButtonLink, Icon, StatusChip, Tooltip, formatRelative } from '../ui'
import { priorityLabel } from '../work-items/constants'
import type { WorkItemPriority } from '../work-items/types'
import type { ProjectAi, Readiness, ReadinessCheck, Suggestion, TaskRef } from './api'
import { projectAiSettingsHref } from './links'
import './ai.css'

function TaskLink({ task }: { task: TaskRef }) {
  return (
    <Link className="aw-ai-task" to={task.href}>
      <span className="aw-ai-task__key">{task.key}</span>
      <span className="aw-ai-task__title">{task.title}</span>
    </Link>
  )
}

const percent = (confidence: number) => `${Math.round(confidence * 100)}%`

/** What a suggestion would change, as a sentence with the tasks linked. */
export function SuggestionSentence({ suggestion }: { suggestion: Suggestion }) {
  const { task, other, value } = suggestion
  const text = (field: string) => (typeof value[field] === 'string' ? (value[field] as string) : '')
  switch (suggestion.kind) {
    case 'blocks':
      return other ? (
        <>
          <TaskLink task={other} /> blocks <TaskLink task={task} />
        </>
      ) : null
    case 'relates_to':
      return other ? (
        <>
          <TaskLink task={task} /> relates to <TaskLink task={other} />
        </>
      ) : null
    case 'duplicates':
      return other ? (
        <>
          <TaskLink task={task} /> duplicates <TaskLink task={other} />
        </>
      ) : null
    case 'parent':
      return other ? (
        <>
          <TaskLink task={task} /> is a subtask of <TaskLink task={other} />
        </>
      ) : null
    case 'blocked_reason':
      return (
        <>
          <TaskLink task={task} /> is blocked: “{text('reason')}”
        </>
      )
    case 'label':
      return (
        <>
          Label <TaskLink task={task} /> <strong>{text('name') || 'with a label'}</strong>
        </>
      )
    case 'priority':
      return (
        <>
          Set <TaskLink task={task} /> to <strong>{priorityLabel(text('priority') as WorkItemPriority)}</strong> priority
        </>
      )
    case 'type':
      return (
        <>
          Make <TaskLink task={task} /> a <strong>{text('name') || 'different type'}</strong>
        </>
      )
    case 'criteria': {
      const criteria = Array.isArray(value.criteria) ? (value.criteria as unknown[]).filter((item) => typeof item === 'string') : []
      return (
        <>
          Add {criteria.length} acceptance {criteria.length === 1 ? 'criterion' : 'criteria'} to <TaskLink task={task} />
          <ul className="aw-ai-criteria">
            {criteria.map((criterion, index) => (
              <li key={index}>{criterion as string}</li>
            ))}
          </ul>
        </>
      )
    }
  }
}

const STATUS_LABELS: Record<Suggestion['status'], string> = {
  pending: 'Waiting for review',
  accepted: 'Accepted',
  dismissed: 'Dismissed',
  applied: 'Applied automatically',
  stale: 'Out of date',
}

/** One suggestion: what it would change, why, what it read, and accept or dismiss. */
export function SuggestionCard({
  busy,
  canEdit,
  onDecide,
  suggestion,
}: {
  busy: boolean
  canEdit: boolean
  onDecide?: (decision: 'accept' | 'dismiss') => void
  suggestion: Suggestion
}) {
  const pending = suggestion.status === 'pending'
  return (
    <li className="aw-ai-suggestion" data-kind={suggestion.kind} data-suggestion-id={suggestion.id} tabIndex={0}>
      <div className="aw-ai-suggestion__body">
        <p className="aw-ai-suggestion__what">
          <SuggestionSentence suggestion={suggestion} />
        </p>
        <p className="aw-ai-suggestion__why">{suggestion.reason}</p>
        <div className="aw-ai-suggestion__meta">
          <Tooltip label={`How sure the model was. Drawn by ${suggestion.model}.`}>
            <span className="aw-ai-confidence" tabIndex={0}>
              <span aria-hidden="true" className="aw-ai-confidence__bar" style={{ width: percent(suggestion.confidence) }} />
              {percent(suggestion.confidence)} sure
            </span>
          </Tooltip>
          {suggestion.personal ? (
            <Tooltip label="It drew on something not everyone in this project can see, so only you see this suggestion.">
              <span tabIndex={0}>
                <StatusChip icon="lock">Only you</StatusChip>
              </span>
            </Tooltip>
          ) : null}
          {suggestion.sources.map((source, index) =>
            source.kind === 'message' ? (
              <Link className="aw-ai-source" key={index} to={source.href}>
                <Icon name="message" size={12} />
                From {source.conversationName}
              </Link>
            ) : source.kind === 'task' && source.task.id !== suggestion.task.id && source.task.id !== suggestion.other?.id ? (
              <Link className="aw-ai-source" key={index} to={source.task.href}>
                <Icon name="checklist" size={12} />
                {source.task.key}
              </Link>
            ) : null,
          )}
          <span className="aw-ai-suggestion__when">{formatRelative(suggestion.createdAt)}</span>
          {!pending ? <StatusChip tone={suggestion.status === 'dismissed' || suggestion.status === 'stale' ? 'muted' : 'success'}>{STATUS_LABELS[suggestion.status]}</StatusChip> : null}
        </div>
      </div>
      {pending && canEdit && onDecide ? (
        <div className="aw-ai-suggestion__actions">
          <Button busy={busy} icon="check" onClick={() => onDecide('accept')} size="sm" variant="primary">
            Accept
          </Button>
          <Button disabled={busy} onClick={() => onDecide('dismiss')} size="sm" variant="ghost">
            Dismiss
          </Button>
        </div>
      ) : null}
    </li>
  )
}

/**
 * The checks that decide whether a task can start, for a person and for an
 * agent. Compact, for lists: only what's missing, on one line, and no "Ready
 * to start" where the list already says so.
 */
export function ReadinessChecks({
  readiness,
  compact = false,
  omit,
}: {
  readiness: Readiness
  compact?: boolean
  /** Checks the list already names once above itself, because every row misses them (compact only). */
  omit?: ReadonlySet<string>
}) {
  const checks = compact ? readiness.checks.filter((check) => !check.ok && !omit?.has(check.id)) : readiness.checks
  if (compact) {
    if (readiness.ready && readiness.readyForAgents) return null
    if (omit?.size && !checks.length) return null
    return (
      <p className="aw-ai-readiness aw-ai-readiness--compact">
        <Icon name={readiness.ready ? 'bot' : 'clock'} size={14} />
        <span>
          <strong>{readiness.ready ? 'Not ready for an agent' : 'Not ready'}</strong>
          {checks.length ? `: ${checks.map((check) => check.label.charAt(0).toLocaleLowerCase() + check.label.slice(1)).join(' · ')}` : ''}
        </span>
      </p>
    )
  }
  return (
    <div className="aw-ai-readiness">
      <div className="aw-ai-readiness__chips">
        <StatusChip icon={readiness.ready ? 'circle-check' : 'clock'} tone={readiness.ready ? 'success' : 'warning'}>
          {readiness.ready ? 'Ready to start' : 'Not ready'}
        </StatusChip>
        <StatusChip icon="bot" tone={readiness.readyForAgents ? 'agent' : 'muted'}>
          {readiness.readyForAgents ? 'An agent can pick it up' : 'Not ready for an agent'}
        </StatusChip>
      </div>
      {checks.length ? (
        <ul className="aw-ai-readiness__checks">
          {checks.map((check) => (
            <li className={check.ok ? 'is-ok' : 'is-missing'} key={check.id}>
              <Icon name={check.ok ? 'check' : check.forAgents ? 'circle-alert' : 'circle-minus'} size={14} />
              {check.label}
            </li>
          ))}
        </ul>
      ) : null}
    </div>
  )
}

/** Says once, above a list, what every task in it is missing, so the rows show only what differs. */
export function SharedReadiness({ checks }: { checks: ReadinessCheck[] }) {
  if (!checks.length) return null
  const forAgents = checks.every((check) => check.forAgents)
  return (
    <p className="aw-ai-readiness aw-ai-readiness--compact aw-ai-readiness--shared" role="note">
      <Icon name={forAgents ? 'bot' : 'clock'} size={14} />
      <span>
        <strong>{forAgents ? 'None of these is ready for an agent yet' : 'None of these is ready to start yet'}</strong>
        {`: ${checks.map((check) => check.label.charAt(0).toLocaleLowerCase() + check.label.slice(1)).join(' · ')}`}
      </span>
    </p>
  )
}

/** Says why suggestions and summaries are off and where to turn them on. */
export function AiNotConfigured({ projectId, status }: { projectId: string; status: ProjectAi }) {
  if (status.configured) return null
  return (
    <Banner
      action={
        <ButtonLink size="sm" to={projectAiSettingsHref(projectId)} variant="secondary">
          Choose a model
        </ButtonLink>
      }
      className="aw-page__banner"
      icon="bot"
      role="note"
      title="No model is set up for this project yet."
      tone="info"
    >
      {status.problem ?? 'Choose a provider and add its key in settings.'} Readiness and the graph still work without one.
    </Banner>
  )
}
