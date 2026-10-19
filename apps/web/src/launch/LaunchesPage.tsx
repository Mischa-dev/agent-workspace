import { Link, useParams } from 'react-router-dom'

import { useAuthenticatedSession } from '../auth/session'
import { ProjectPageError, ProjectPageLoading } from '../projects/ProjectPageStates'
import { useProject } from '../projects/hooks'
import { AppShell } from '../shell/AppShell'
import { Button, ButtonLink, EmptyState, ErrorState, ListSkeleton, PageHeader, Row, RowList, Section, StatusChip, describeError, formatDateTime, formatRelative, showToast } from '../ui'
import { workItemHref, workItemsHref } from '../work-items/links'
import type { Launch } from './api'
import { useCancelLaunch, useLaunchList } from './hooks'
import { launchStatus, threadHref } from './launchModel'
import './launch.css'

const START_LABELS: Record<string, string> = {
  now: 'As soon as there’s room',
  window_reset: 'When the usage window resets',
  blockers_done: 'When blockers finish',
}

/** The project's runs: what waits to start and why, in the order it will start, and what ran lately. */
export function LaunchesPage() {
  const session = useAuthenticatedSession()
  const { projectId = '' } = useParams()
  const project = useProject(projectId)
  const launches = useLaunchList(projectId)
  const cancel = useCancelLaunch(projectId)

  if (project.isPending) {
    return (
      <AppShell user={session.user}>
        <ProjectPageLoading label="Loading runs" />
      </AppShell>
    )
  }
  if (project.isError) {
    return (
      <AppShell user={session.user}>
        <ProjectPageError error={project.error} onRetry={() => void project.refetch()} />
      </AppShell>
    )
  }

  const cancelLaunch = (launch: Launch) =>
    cancel.mutate(launch.id, {
      onSuccess: () => showToast({ title: `${launch.workItem?.key ?? 'The task'} won’t start`, tone: 'success' }),
      onError: (error) =>
        showToast({ tone: 'danger', title: `${launch.workItem?.key ?? 'The task'} is still queued`, description: describeError(error).message }),
    })

  const row = (launch: Launch) => {
    const status = launchStatus(launch)
    const task = launch.workItem
    return (
      <Row
        actions={
          <>
            {launch.conversationId ? (
              <ButtonLink size="sm" to={threadHref(projectId, launch.conversationId)} variant="ghost">
                Thread
              </ButtonLink>
            ) : null}
            {launch.agentRunId ? (
              <ButtonLink size="sm" to={`/runs/${encodeURIComponent(launch.agentRunId)}`} variant="ghost">
                Run details
              </ButtonLink>
            ) : null}
            {launch.canCancel ? (
              <Button
                busy={cancel.isPending && cancel.variables === launch.id}
                onClick={() => cancelLaunch(launch)}
                size="sm"
                variant="ghost"
              >
                Don’t start
              </Button>
            ) : null}
          </>
        }
        key={launch.id}
        leading={
          <StatusChip icon={status.icon} spin={status.spin} tone={status.tone}>
            {status.label}
          </StatusChip>
        }
        meta={
          <>
            {[launch.harnessName ?? 'Agent', launch.environmentName, `by ${launch.requestedByName}`, whenLabel(launch)]
              .filter(Boolean)
              .join(' · ')}
          </>
        }
        title={
          task ? (
            <Link to={workItemHref(projectId, task.id)}>
              {task.key} · {task.title}
            </Link>
          ) : (
            'A task you can’t see'
          )
        }
      >
        {launch.statusReason ? <p className="aw-launches__reason">{launch.statusReason}</p> : null}
        {launch.state === 'handed_off' && !launch.statusReason ? (
          <p className="aw-launches__reason">Handed to its runner. The runner’s agent answers in the task’s thread.</p>
        ) : null}
      </Row>
    )
  }

  const empty = launches.data && !launches.data.queued.length && !launches.data.recent.length
  return (
    <AppShell project={project.data} user={session.user}>
      <div className="aw-page aw-page--wide">
        <PageHeader icon="activity" title="Runs">
          Agents working on tasks in {project.data.name}: what’s waiting to start, and what ran lately.
        </PageHeader>
        {launches.isPending ? <ListSkeleton label="Loading runs" /> : null}
        {launches.isError ? <ErrorState error={launches.error} onRetry={() => void launches.refetch()} title="Runs didn’t load" /> : null}
        {empty ? (
          <EmptyState action={<ButtonLink to={workItemsHref(projectId)}>Open tasks</ButtonLink>} icon="activity" title="No runs yet">
            Launch tasks from the task list or a task’s page. Ones that can’t start yet wait here, in order.
          </EmptyState>
        ) : launches.data ? (
          <>
            <Section
              description={
                launches.data.queued.length > 1 ? 'In the order they start: unblocked tasks first, as each account has room.' : undefined
              }
              id="launch-queue-heading"
              title={`Waiting (${launches.data.queued.length})`}
            >
              {launches.data.queued.length ? (
                <RowList label="Waiting to start">{launches.data.queued.map(row)}</RowList>
              ) : (
                <p className="aw-quiet">Nothing is waiting. Tasks that can’t start yet wait here.</p>
              )}
            </Section>
            <Section id="launch-recent-heading" title="Recent">
              {launches.data.recent.length ? (
                <RowList label="Recent runs">{launches.data.recent.map(row)}</RowList>
              ) : (
                <p className="aw-quiet">Nothing has run yet.</p>
              )}
            </Section>
          </>
        ) : null}
      </div>
    </AppShell>
  )
}

function whenLabel(launch: Launch) {
  if (launch.state !== 'queued') return formatRelative(launch.startedAt ?? launch.createdAt)
  if (launch.startWhen === 'at' && launch.startAt) return `Starts ${formatDateTime(launch.startAt)}`
  return START_LABELS[launch.startWhen] ?? ''
}
