import { useMutation } from '@tanstack/react-query'
import { Link } from 'react-router-dom'

import { testProjectAi } from '../../ai/api'
import { useAnalyze, useProjectAi } from '../../ai/hooks'
import { aiErrorMessage, planningHref } from '../../ai/links'
import type { Project } from '../../projects/types'
import { Banner, Button, DetailList, ErrorState, Section, SettingsSkeleton, StatusChip, formatRelative, showToast } from '../../ui'
import { ScopedSettings } from '../ScopedSettings'
import { SUGGESTION_KEYS, SUGGESTION_UNSET_LABELS, here } from './options'
import './runs.css'

const KEY_SOURCES = { project: 'This project’s key', workspace: 'The workspace key' } as const

/** A project's suggestions and summaries: what they use, a test, recent jobs, and the settings, each following the workspace. */
export function ProjectSuggestions({ project, workspaceHref }: { project: Project; workspaceHref?: string }) {
  const status = useProjectAi(project.id)
  const analyze = useAnalyze(project.id)
  const test = useMutation({
    mutationFn: () => testProjectAi(project.id),
    onSuccess: (result) => showToast({ tone: 'success', title: `${result.model} answered in ${result.latencyMs} ms` }),
  })
  const data = status.data

  return (
    <Section
      actions={
        data?.configured ? (
          <span className="aw-runs__actions">
            <Button busy={test.isPending} onClick={() => test.mutate()} size="sm">
              Test connection
            </Button>
            {data.canEdit ? (
              <Button
                busy={analyze.isPending}
                icon="bot"
                onClick={() => analyze.mutate(undefined, { onSuccess: () => showToast({ tone: 'success', title: 'Analyzing the project' }) })}
                size="sm"
              >
                Analyze now
              </Button>
            ) : null}
          </span>
        ) : null
      }
      description="What dependency detection, triage, plan import and summaries use here."
      id="suggestions"
      title="Suggestions and summaries"
    >
      {status.isPending ? <SettingsSkeleton label="Loading suggestions and summaries" sections={1} /> : null}
      {status.isError ? (
        <ErrorState action={<Button onClick={() => void status.refetch()}>Try again</Button>} title="Suggestions and summaries didn’t load">
          Check your connection and try again.
        </ErrorState>
      ) : null}
      {data && !data.configured ? (
        <Banner icon="bot" role="note" title="Not set up for this project." tone="info">
          {data.problem ? here(data.problem) : 'Choose a provider and add its key.'} A workspace admin adds keys in{' '}
          {workspaceHref ? <Link to={`${workspaceHref}#suggestions`}>workspace settings</Link> : 'workspace settings'}.
        </Banner>
      ) : null}
      {test.isError ? <Banner tone="danger">{aiErrorMessage(test.error, 'The test')}</Banner> : null}
      {analyze.isError ? <Banner tone="danger">{aiErrorMessage(analyze.error, 'Analyzing')}</Banner> : null}
      {data?.configured ? (
        <DetailList
          items={[
            { label: 'Provider', value: data.provider?.displayName ?? '—' },
            { label: 'Quick jobs', value: <code className="aw-inline-code">{data.quickModel}</code> },
            { label: 'Deep jobs', value: <code className="aw-inline-code">{data.deepModel}</code> },
            { label: 'Key', value: data.keySource ? KEY_SOURCES[data.keySource] : 'Not needed' },
            {
              label: 'Suggestions',
              value: (
                <Link to={planningHref(project.id, 'suggestions')}>
                  {data.pendingSuggestions ? `${data.pendingSuggestions} to review` : 'Nothing to review'}
                </Link>
              ),
            },
          ]}
        />
      ) : null}
      {data?.runs.length ? (
        <ul aria-label="Recent jobs" className="aw-ai-runs">
          {data.runs.slice(0, 6).map((run) => (
            <li key={run.id}>
              <StatusChip dots={run.state === 'running'} tone={run.state === 'failed' ? 'danger' : run.state === 'succeeded' ? 'success' : 'agent'}>
                {run.kind}
              </StatusChip>
              <span>
                {run.state === 'failed'
                  ? (run.error ?? 'Failed')
                  : run.state === 'succeeded'
                    ? `${run.tasksConsidered} tasks checked, ${run.suggestionsCreated} suggestions${run.suggestionsApplied ? `, ${run.suggestionsApplied} applied` : ''}`
                    : run.state === 'running'
                      ? 'Running'
                      : 'Waiting to run'}
              </span>
              <span className="aw-quiet">{formatRelative(run.finishedAt ?? run.createdAt)}</span>
            </li>
          ))}
        </ul>
      ) : null}
      <ScopedSettings
        prefixes={SUGGESTION_KEYS}
        readOnlyNote={false}
        target={{ scope: 'project', organizationId: project.organizationId, projectId: project.id }}
        unsetLabels={SUGGESTION_UNSET_LABELS}
        workspaceHref={workspaceHref}
      />
      <p className="aw-quiet">Applying suggestions automatically acts as whoever turned it on, and stops if they lose access to the project.</p>
    </Section>
  )
}
