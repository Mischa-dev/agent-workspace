import { Link } from 'react-router-dom'

import { Button, ButtonLink, ErrorState, ListSkeleton, Row, RowList, Section, StatusChip, describeError, formatDateTime, formatRelative, showToast } from '../ui'
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

function whenLabel(launch: Launch) {
  if (launch.state !== 'queued') return formatRelative(launch.startedAt ?? launch.createdAt)
  if (launch.startWhen === 'at' && launch.startAt) return `Starts ${formatDateTime(launch.startAt)}`
  return START_LABELS[launch.startWhen] ?? ''
}

/**
 * A project's launches that aren't runs (yet): what waits to start and why,
 * in the order it will start, and lately ones that never ran here.
 */
export function LaunchQueue({ projectId }: { projectId: string }) {
  const launches = useLaunchList(projectId)
  const cancel = useCancelLaunch(projectId)

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
            {launch.canCancel ? (
              <Button busy={cancel.isPending && cancel.variables === launch.id} onClick={() => cancelLaunch(launch)} size="sm" variant="ghost">
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
        meta={[launch.harnessName ?? 'Agent', launch.environmentName, `by ${launch.requestedByName}`, whenLabel(launch)].filter(Boolean).join(' · ')}
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

  if (launches.isPending) return <ListSkeleton label="Loading launches" rows={2} />
  if (launches.isError) return <ErrorState error={launches.error} onRetry={() => void launches.refetch()} title="Launches didn’t load" />
  const queued = launches.data.queued
  // Started launches are the runs below; the rest never ran here.
  const other = launches.data.recent.filter((launch) => !launch.agentRunId && launch.state !== 'started')
  return (
    <>
      <Section
        description={queued.length > 1 ? 'In the order they start: unblocked tasks first, as each account has room.' : undefined}
        id="launch-queue-heading"
        title={`Waiting to start (${queued.length})`}
      >
        {queued.length ? (
          <RowList label="Waiting to start">{queued.map(row)}</RowList>
        ) : (
          <p className="aw-quiet">Nothing is waiting. Tasks that can’t start yet wait here, in order.</p>
        )}
      </Section>
      {other.length ? (
        <Section
          description="Launched lately but never ran here: cancelled, couldn’t start, or went to a connected runner."
          id="launch-other-heading"
          title="Didn’t run here"
        >
          <RowList label="Launches that didn’t run here">{other.map(row)}</RowList>
        </Section>
      ) : null}
    </>
  )
}
