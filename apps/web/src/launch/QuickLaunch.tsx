import { useNavigate } from 'react-router-dom'

import { Button, describeError, showToast } from '../ui'
import { useCreateLaunches } from './hooks'
import { launchSummary, launchesHref, threadHref } from './launchModel'

/**
 * Launch for a task listed outside its own page, such as a capacity alert's ready tasks: one click starts it on the
 * project's defaults, on `providerAccountId` when given.
 */
export function QuickLaunch({
  providerAccountId,
  task,
}: {
  providerAccountId?: string
  task: { id: string; key: string; projectId: string }
}) {
  const navigate = useNavigate()
  const create = useCreateLaunches(task.projectId)
  const launch = () =>
    create.mutate(
      { input: { workItemIds: [task.id], providerAccountId }, idempotencyKey: crypto.randomUUID() },
      {
        onSuccess: ({ launches }) => {
          const first = launches[0]
          showToast({
            ...launchSummary(launches),
            action: first?.conversationId
              ? { label: 'Open thread', onSelect: () => navigate(threadHref(task.projectId, first.conversationId ?? '')) }
              : { label: 'View runs', onSelect: () => navigate(launchesHref(task.projectId)) },
          })
        },
        onError: (error) =>
          showToast({ tone: 'danger', title: `${task.key} didn’t launch`, description: describeError(error).message }),
      },
    )
  return (
    <Button
      aria-label={`Launch ${task.key}`}
      busy={create.isPending}
      disabled={create.isSuccess}
      icon="play"
      onClick={launch}
      size="sm"
      variant="primary"
    >
      {create.isSuccess ? 'Launched' : 'Launch'}
    </Button>
  )
}
