import { useState } from 'react'
import { useMutation } from '@tanstack/react-query'

import { Banner, Button } from '../ui'
import { summarizeTask, type Summary } from './api'
import { useDecideSuggestion, useDraftCriteria, useProjectAi, useSuggestions, useTriage } from './hooks'
import { SuggestionCard } from './parts'
import { aiErrorMessage } from './links'
import { SummaryView } from './SummaryView'

/**
 * Suggestions about a task to accept or dismiss, and on-demand triage,
 * criteria and a catch-up summary. Whether the task can start is shown next
 * to its launch button instead.
 */
export default function TaskAiPanel({
  disabled,
  projectId,
  taskId,
}: {
  disabled: boolean
  projectId: string
  taskId: string
}) {
  const status = useProjectAi(projectId)
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

  // Nothing to show until suggestions are set up for the project or one is waiting.
  if (!configured && !pending.length) return null
  return (
    <section aria-labelledby={`suggestions-${taskId}`} className="aw-tsection aw-ai-taskpanel">
      <header className="aw-tsection__head">
        <h2 className="aw-tsection__title" id={`suggestions-${taskId}`}>
          Suggestions
          {pending.length ? <span className="aw-tsection__count">{pending.length}</span> : null}
        </h2>
        {configured ? (
          <div className="aw-ai-taskpanel__actions">
            {canAct ? (
              <>
                <Button
                  busy={triage.isPending}
                  icon="bot"
                  onClick={() => triage.mutate(taskId, { onSuccess: () => setNotice('Looking at this task. Suggestions show up here and in Suggestions.') })}
                  size="sm"
                  variant="ghost"
                >
                  Suggest labels and links
                </Button>
                <Button busy={criteria.isPending} icon="checklist" onClick={() => criteria.mutate(taskId)} size="sm" variant="ghost">
                  Draft criteria
                </Button>
              </>
            ) : null}
            <Button busy={summarize.isPending} icon="activity" onClick={() => summarize.mutate()} size="sm" variant="ghost">
              Catch me up
            </Button>
          </div>
        ) : null}
      </header>
      {pending.length ? (
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
      ) : null}
      {notice ? (
        <p aria-live="polite" className="aw-quiet">
          {notice}
        </p>
      ) : null}
      {error ? <Banner tone="danger">{aiErrorMessage(error)}</Banner> : null}
      {summary ? <SummaryView summary={summary} /> : null}
    </section>
  )
}
