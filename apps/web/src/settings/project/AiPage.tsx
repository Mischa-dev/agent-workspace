import { useMutation } from '@tanstack/react-query'
import { Link } from 'react-router-dom'

import { testProjectAi } from '../../ai/api'
import { planningHref } from '../../ai/links'
import { useAnalyze, useProjectAi } from '../../ai/hooks'
import { aiErrorMessage } from '../../ai/links'
import { Banner, Button, DetailList, ErrorState, PageHeader, Section, SettingsSkeleton, StatusChip, countLabel, formatFullDate, formatRelative, showToast } from '../../ui'
import { workspaceSettingsHref } from '../paths'
import { ScopedSettings } from '../ScopedSettings'
import type { SettingsPageProps } from '../types'

const KEY_SOURCES = { project: 'This project’s key', workspace: 'The workspace key' } as const

const RUN_KINDS: Record<string, string> = { edit: 'Edit', analyze: 'Analysis', nightly: 'Nightly check', triage: 'Triage' }

/** Project › Models: what this project's models use, the switches for detection and applying, and recent runs. */
export default function AiPage({ project }: SettingsPageProps) {
  const projectId = project?.id ?? ''
  const status = useProjectAi(projectId)
  const analyze = useAnalyze(projectId)
  const test = useMutation({
    mutationFn: () => testProjectAi(projectId),
    onSuccess: (result) => showToast({ tone: 'success', title: `${result.model} answered in ${result.latencyMs} ms` }),
  })
  if (!project) return null
  const data = status.data

  return (
    <>
      <PageHeader icon="star" title="Models">
        What {project.name} uses for dependency detection, triage, plan import and summaries. Each setting follows the{' '}
        <Link to={workspaceSettingsHref('ai')}>workspace</Link> until you override it here.
      </PageHeader>
      {status.isPending ? <SettingsSkeleton label="Loading model status" sections={1} /> : null}
      {status.isError ? <ErrorState error={status.error} onRetry={() => void status.refetch()} title="Model status didn’t load" /> : null}
      {data ? (
        <Section
          actions={
            data.configured ? (
              <span className="aw-wrap">
                <Button busy={test.isPending} onClick={() => test.mutate()} size="sm">
                  Test connection
                </Button>
                {data.canEdit ? (
                  <Button busy={analyze.isPending} icon="bot" onClick={() => analyze.mutate(undefined, { onSuccess: () => showToast({ tone: 'success', title: 'Analyzing the project' }) })} size="sm">
                    Analyze now
                  </Button>
                ) : null}
              </span>
            ) : null
          }
          id="ai-project-status"
          title="Status"
        >
          {data.configured ? null : (
            <Banner icon="bot" role="note" title="Models aren’t set up for this project." tone="info">
              {data.problem ?? 'Choose a provider and add its key.'} A workspace admin can add keys in{' '}
              <Link to={workspaceSettingsHref('ai')}>workspace settings</Link>.
            </Banner>
          )}
          {test.isError ? <Banner tone="danger">{aiErrorMessage(test.error, 'The test')}</Banner> : null}
          {analyze.isError ? <Banner tone="danger">{aiErrorMessage(analyze.error, 'Analyzing')}</Banner> : null}
          {data.configured ? (
            <DetailList
              items={[
                { label: 'Provider', value: data.provider?.displayName ?? '—' },
                { label: 'Quick model', value: <code className="aw-inline-code">{data.quickModel}</code> },
                { label: 'Deep model', value: <code className="aw-inline-code">{data.deepModel}</code> },
                { label: 'Key', value: data.keySource ? KEY_SOURCES[data.keySource] : 'Not needed' },
                {
                  label: 'Suggestions',
                  value: <Link to={planningHref(projectId, 'suggestions')}>{data.pendingSuggestions ? `${data.pendingSuggestions} to review` : 'Nothing to review'}</Link>,
                },
              ]}
            />
          ) : null}
          {data.runs.length ? (
            <ul aria-label="Recent runs" className="aw-ai-runs">
              {data.runs.slice(0, 6).map((run) => (
                <li key={run.id}>
                  <StatusChip tone={run.state === 'failed' ? 'danger' : run.state === 'succeeded' ? 'success' : 'agent'} dots={run.state === 'running'}>
                    {RUN_KINDS[run.kind] ?? 'Run'}
                  </StatusChip>
                  <span>
                    {run.state === 'failed'
                      ? (run.error ?? 'Failed')
                      : run.state === 'succeeded'
                        ? `${countLabel(run.tasksConsidered, 'task')} checked, ${countLabel(run.suggestionsCreated, 'suggestion')}${run.suggestionsApplied ? `, ${run.suggestionsApplied} applied` : ''}`
                        : run.state === 'running'
                          ? 'Running'
                          : 'Waiting to run'}
                  </span>
                  <span className="aw-quiet" title={formatFullDate(run.finishedAt ?? run.createdAt)}>
                    {formatRelative(run.finishedAt ?? run.createdAt)}
                  </span>
                </li>
              ))}
            </ul>
          ) : null}
        </Section>
      ) : null}
      <Section
        description="Applying automatically acts as whoever turned it on, and stops if they lose access to the project."
        id="ai-project-settings"
        title="Settings"
      >
        <ScopedSettings prefixes={['ai.']} target={{ scope: 'project', organizationId: project.organizationId, projectId: project.id }} />
      </Section>
    </>
  )
}
