import { Link, useParams } from 'react-router-dom'

import { isApiError } from '../api/errors'
import { useAuthenticatedSession } from '../auth/session'
import { ProjectPageError, ProjectPageLoading } from '../projects/ProjectPageStates'
import { useProject } from '../projects/hooks'
import { AppShell } from '../shell/AppShell'
import { Button, EmptyState, ErrorState, ListSkeleton, PageHeader, Row, RowList, Section, StatusChip, formatDateTime, formatRelative, showToast } from '../ui'
import { workItemHref } from '../work-items/links'
import type { Launch } from './api'
import { useCancelLaunch, useLaunchList } from './hooks'
import { launchStatus, threadHref } from './launchModel'
import './launch.css'

const START_LABELS: Record<string, string> = {
  now: 'As soon as there’s room',
  window_reset: 'When the usage window resets',
  blockers_done: 'When blockers finish',
}

/** The project's launch queue: what waits and why, in the order it will start, and what started lately. */
export function LaunchesPage() {
  const session = useAuthenticatedSession()
  const { projectId = '' } = useParams()
  const project = useProject(projectId)
  const launches = useLaunchList(projectId)
  const cancel = useCancelLaunch(projectId)

  if (project.isPending) {
    return (
      <AppShell user={session.user}>
        <ProjectPageLoading label="Loading launches" />
      </AppShell>
    )
  }
  if (project.isError) {
    return (
      <AppShell user={session.user}>
        <ProjectPageError onRetry={() => void project.refetch()} />
      </AppShell>
    )
  }

  const cancelLaunch = (launch: Launch) =>
    cancel.mutate(launch.id, {
      onSuccess: () => showToast({ title: `${launch.workItem?.key ?? 'The launch'} won’t start.` }),
      onError: (error) =>
        showToast({ tone: 'danger', title: 'That launch wasn’t cancelled.', description: isApiError(error) ? error.message : undefined }),
    })

  const row = (launch: Launch) => {
    const status = launchStatus(launch)
    const task = launch.workItem
    return (
      <Row
        actions={
          <>
            {launch.conversationId ? (
              <Link className="aw-btn aw-btn--ghost aw-btn--sm" to={threadHref(projectId, launch.conversationId)}>
                Thread
              </Link>
            ) : null}
            {launch.canCancel ? (
              <Button
                busy={cancel.isPending && cancel.variables === launch.id}
                onClick={() => cancelLaunch(launch)}
                size="sm"
                variant="ghost"
              >
                Cancel
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
      </Row>
    )
  }

  return (
    <AppShell project={project.data} user={session.user}>
      <div className="aw-page aw-page--wide">
        <PageHeader icon="clock" title="Launches">
          Tasks waiting for an agent, in the order they start. Unblocked tasks go first, and each account takes as many as
          it has room for.
        </PageHeader>
        {launches.isPending ? <ListSkeleton /> : null}
        {launches.isError ? (
          <ErrorState
            action={
              <Button onClick={() => void launches.refetch()} size="sm">
                Retry
              </Button>
            }
            title="The launch queue didn’t load."
          />
        ) : null}
        {launches.data ? (
          <>
            <Section id="launch-queue-heading" title={`Waiting (${launches.data.queued.length})`}>
              {launches.data.queued.length ? (
                <RowList label="Waiting launches">{launches.data.queued.map(row)}</RowList>
              ) : (
                <EmptyState icon="clock" title="Nothing is waiting">
                  Launch tasks from the task list or a task’s page. Ones that can’t start yet wait here.
                </EmptyState>
              )}
            </Section>
            <Section id="launch-recent-heading" title="Recent">
              {launches.data.recent.length ? (
                <RowList label="Recent launches">{launches.data.recent.map(row)}</RowList>
              ) : (
                <EmptyState icon="play" title="No launches yet" />
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
