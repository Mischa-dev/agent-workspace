import { useState } from 'react'
import { useMutation } from '@tanstack/react-query'
import { Link } from 'react-router-dom'

import { Banner, Button } from '../ui'
import { summarizeTask, type Summary } from './api'
import { useDecideSuggestion, useDraftCriteria, useProjectAi, useReadiness, useSuggestions, useTriage } from './hooks'
import { ReadinessChecks, SuggestionCard } from './parts'
import { aiErrorMessage, planningHref } from './links'
import { SummaryView } from './SummaryView'

/**
 * The AI side of a task: whether it can start (for a person and for an
 * agent), suggestions about it to accept or dismiss, and on-demand triage,
 * criteria and a catch-up summary.
 */
export default function TaskAiPanel({
  disabled,
  projectId,
  taskId,
  version,
}: {
  disabled: boolean
  projectId: string
  taskId: string
  version: number
}) {
  const status = useProjectAi(projectId)
  const readiness = useReadiness(projectId, taskId, version)
  const suggestions = useSuggestions(projectId, { workItemId: taskId, status: 'pending' })
  const decide = useDecideSuggestion(projectId)
  const triage = useTriage(projectId)
  const criteria = useDraftCriteria(projectId)
  const [summary, setSummary] = useState<Summary | null>(null)
  const summarize = useMutation({
    mutationFn: () => summarizeTask(projectId, taskId),
    onSuccess: (result) => setSummary(result.summary),
  })
  const [notice, setNotice] = useState<string | null>(null)
  const error = decide.error ?? triage.error ?? criteria.error ?? summarize.error
  const configured = status.data?.configured ?? false
  const canAct = configured && !disabled && (status.data?.canEdit ?? false)
  const pending = suggestions.data?.suggestions ?? []

  return (
    <div className="aw-sidecard aw-ai-taskpanel">
      <h2 className="aw-sidecard__title">Readiness</h2>
      {readiness.data ? <ReadinessChecks readiness={readiness.data} /> : readiness.isError ? <p className="aw-quiet">Readiness didn’t load.</p> : null}
      {pending.length ? (
        <>
          <h2 className="aw-sidecard__title">
            Suggestions <span className="aw-quiet">({pending.length})</span>
          </h2>
          <ul aria-label="Suggestions for this task" className="aw-ai-suggestions aw-ai-suggestions--compact">
            {pending.map((suggestion) => (
              <SuggestionCard
                busy={decide.isPending && decide.variables?.suggestionId === suggestion.id}
                canEdit={!disabled && (suggestions.data?.canEdit ?? false)}
                key={suggestion.id}
                onDecide={(decision) => decide.mutate({ suggestionId: suggestion.id, decision })}
                suggestion={suggestion}
              />
            ))}
          </ul>
        </>
      ) : null}
      {configured ? (
        <div className="aw-ai-taskpanel__actions">
          {canAct ? (
            <>
              <Button
                busy={triage.isPending}
                icon="bot"
                onClick={() => triage.mutate(taskId, { onSuccess: () => setNotice('Looking at this task. Suggestions show up here and in Suggestions.') })}
                size="sm"
              >
                Suggest labels and links
              </Button>
              <Button busy={criteria.isPending} icon="checklist" onClick={() => criteria.mutate(taskId)} size="sm">
                Draft criteria
              </Button>
            </>
          ) : null}
          <Button busy={summarize.isPending} icon="activity" onClick={() => summarize.mutate()} size="sm">
            Catch me up
          </Button>
        </div>
      ) : status.isSuccess ? (
        <p className="aw-quiet">
          AI isn’t set up for this project. <Link to={planningHref(projectId, 'next')}>Up next</Link> still shows what can start.
        </p>
      ) : null}
      {notice ? (
        <p aria-live="polite" className="aw-quiet">
          {notice}
        </p>
      ) : null}
      {error ? <Banner tone="danger">{aiErrorMessage(error)}</Banner> : null}
      {summary ? <SummaryView summary={summary} /> : null}
    </div>
  )
}
