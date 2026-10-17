import { useRef, useState, type MouseEvent } from 'react'
import { Link, useNavigate } from 'react-router-dom'

import { isApiError } from '../api/errors'
import { Button, IconButton, Menu, StatusChip, formatRelative, showToast } from '../ui'
import type { Launch } from './api'
import { useCreateLaunches, useLaunchPlan } from './hooks'
import { LazyLaunchDialog as LaunchDialog } from './LazyLaunchDialog'
import { launchStatus, launchSummary, launchesHref, threadHref } from './launchModel'
import './launch.css'

/**
 * Launch on a task: one click starts an agent on the project's defaults, shift-click or the menu opens every choice.
 * Shows the latest launch and where its thread is.
 */
export function LaunchCard({ item }: { item: { id: string; key: string; title: string; projectId: string } }) {
  const navigate = useNavigate()
  const plan = useLaunchPlan(item.projectId, item.id)
  const create = useCreateLaunches(item.projectId)
  const [dialogOpen, setDialogOpen] = useState(false)
  const launchButton = useRef<HTMLButtonElement>(null)
  const data = plan.data
  const environment = data?.environments.find((candidate) => candidate.id === data.environmentId)
  const harness = data?.harnesses.find((candidate) => candidate.kind === data.harness)
  const accounts = (data?.accounts ?? []).filter((account) => account.harness === data?.harness)
  const latest = data?.launches[0]
  const blocked = (data?.blockers.length ?? 0) > 0
  const unavailable = !data ? null : !harness ? 'No agent is set up for this project yet.' : accounts.length === 0 ? `Connect a ${harness.displayName} account in Settings › Provider accounts first.` : null

  const launched = (launches: Launch[]) => {
    setDialogOpen(false)
    const summary = launchSummary(launches)
    const first = launches[0]
    showToast({
      ...summary,
      action:
        first?.conversationId && launches.length === 1
          ? { label: 'Open thread', onSelect: () => navigate(threadHref(item.projectId, first.conversationId ?? '')) }
          : { label: 'View queue', onSelect: () => navigate(launchesHref(item.projectId)) },
    })
  }

  const launchNow = (event: MouseEvent<HTMLButtonElement>) => {
    if (event.shiftKey || !data) {
      setDialogOpen(true)
      return
    }
    create.mutate(
      { input: { workItemIds: [item.id], environmentId: data.environmentId ?? undefined }, idempotencyKey: crypto.randomUUID() },
      {
        onSuccess: (result) => launched(result.launches),
        onError: (error) =>
          showToast({ tone: 'danger', title: `${item.key} didn’t launch.`, description: isApiError(error) ? error.message : undefined }),
      },
    )
  }

  if (plan.isError && isApiError(plan.error) && (plan.error.status === 403 || plan.error.status === 404)) return null

  return (
    <section aria-labelledby={`launch-heading-${item.id}`} className="aw-sidecard aw-launchcard">
      <h2 className="aw-sidecard__title" id={`launch-heading-${item.id}`}>
        Launch an agent
      </h2>
      <p className="aw-sidecard__text">
        {plan.isPending
          ? 'Loading the project’s defaults…'
          : plan.isError
            ? 'The launch options didn’t load.'
            : unavailable ??
              `${harness?.displayName}${environment ? ` in ${environment.name}` : ''}, on the project’s defaults. It works in the task’s thread.`}
      </p>
      {blocked ? (
        <p className="aw-sidecard__note">Waiting on {data?.blockers.map((blocker) => blocker.key).join(', ')}.</p>
      ) : null}
      <div className="aw-launchcard__actions">
        <Button
          busy={create.isPending}
          disabled={plan.isPending || Boolean(unavailable) || plan.isError}
          icon="play"
          onClick={launchNow}
          ref={launchButton}
          title="Launch with the defaults (Shift-click to choose)"
          variant="primary"
        >
          Launch
        </Button>
        <Menu
          items={[
            { label: 'Choose options…', icon: 'settings', onSelect: () => setDialogOpen(true) },
            { label: 'Launch queue', icon: 'clock', onSelect: () => navigate(launchesHref(item.projectId)) },
          ]}
          label="More launch options"
          trigger={<IconButton icon="chevron-down" label="More launch options" size="sm" />}
        />
      </div>
      {latest ? <LatestLaunch launch={latest} projectId={item.projectId} /> : null}
      {dialogOpen ? (
        <LaunchDialog
          onClose={() => setDialogOpen(false)}
          onLaunched={launched}
          projectId={item.projectId}
          returnFocus={launchButton}
          tasks={[{ id: item.id, key: item.key, title: item.title }]}
        />
      ) : null}
    </section>
  )
}

function LatestLaunch({ launch, projectId }: { launch: Launch; projectId: string }) {
  const status = launchStatus(launch)
  return (
    <div className="aw-launchcard__latest">
      <StatusChip icon={status.icon} spin={status.spin} tone={status.tone}>
        {status.label}
      </StatusChip>
      <span className="aw-sidecard__note">
        {launch.harnessName ?? 'Agent'} · {formatRelative(launch.startedAt ?? launch.createdAt)}
      </span>
      {launch.statusReason && launch.state !== 'started' ? <p className="aw-sidecard__note">{launch.statusReason}</p> : null}
      {launch.conversationId ? (
        <Link className="aw-launchcard__link" to={threadHref(projectId, launch.conversationId)}>
          Open thread
        </Link>
      ) : null}
    </div>
  )
}
