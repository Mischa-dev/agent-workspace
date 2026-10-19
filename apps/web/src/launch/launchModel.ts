import { countLabel, type ChipTone, type IconName } from '../ui'
import type { Launch, SettingSource } from './api'

/** The server takes this many tasks in one launch. */
export const MAX_TASKS_PER_LAUNCH = 50

export function sourceLabel(source: SettingSource) {
  switch (source) {
    case 'launch':
      return 'this launch’s'
    case 'project':
      return 'project'
    case 'user':
      return 'your'
    case 'workspace':
      return 'workspace'
    default:
      return 'built-in'
  }
}

const RUN_LABELS: Record<string, { label: string; tone: ChipTone; icon: IconName; spin?: boolean }> = {
  created: { label: 'Starting', tone: 'agent', icon: 'loader', spin: true },
  queued: { label: 'Starting', tone: 'agent', icon: 'loader', spin: true },
  starting: { label: 'Starting', tone: 'agent', icon: 'loader', spin: true },
  working: { label: 'Working', tone: 'agent', icon: 'loader', spin: true },
  blocked: { label: 'Needs input', tone: 'warning', icon: 'triangle-alert' },
  recovering: { label: 'Reconnecting', tone: 'warning', icon: 'loader', spin: true },
  completed: { label: 'Done', tone: 'success', icon: 'circle-check' },
  failed: { label: 'Failed', tone: 'danger', icon: 'circle-alert' },
  canceled: { label: 'Stopped', tone: 'muted', icon: 'stop' },
  interrupted: { label: 'Interrupted', tone: 'warning', icon: 'triangle-alert' },
}

/** One chip for where a launch is: waiting, its run's state once started, or why it never ran. */
export function launchStatus(launch: Launch): { label: string; tone: ChipTone; icon: IconName; spin?: boolean } {
  if (launch.state === 'queued') return { label: 'Queued', tone: 'muted', icon: 'clock' }
  if (launch.state === 'cancelled') return { label: 'Canceled', tone: 'muted', icon: 'x' }
  if (launch.state === 'failed') return { label: 'Didn’t start', tone: 'danger', icon: 'circle-alert' }
  if (launch.state === 'handed_off') return { label: 'Handed off', tone: 'agent', icon: 'send' }
  return (launch.runState && RUN_LABELS[launch.runState]) || { label: 'Started', tone: 'agent', icon: 'play' }
}

export function threadHref(projectId: string, conversationId: string) {
  return `/projects/${encodeURIComponent(projectId)}/threads/${encodeURIComponent(conversationId)}`
}

export function launchesHref(projectId: string) {
  return `/projects/${encodeURIComponent(projectId)}/launches`
}

/** What to say after a launch request: started, queued and why, or didn't start and why. Toast titles, so no full stop. */
export function launchSummary(launches: Launch[]): { tone: 'success' | 'info' | 'danger'; title: string; description?: string } {
  if (launches.length === 1) {
    const [launch] = launches
    const key = launch.workItem?.key ?? 'The task'
    if (launch.state === 'started') return { tone: 'success', title: `${key} started on ${launch.harnessName ?? 'the agent'}` }
    if (launch.state === 'queued') return { tone: 'info', title: `${key} is queued`, description: launch.statusReason ?? undefined }
    if (launch.state === 'handed_off') return { tone: 'success', title: `${key} was handed to ${launch.harnessName ?? 'its runner'}`, description: 'Its agent answers in the task’s thread.' }
    if (launch.state === 'failed') return { tone: 'danger', title: `${key} didn’t start`, description: launch.statusReason ?? undefined }
    return { tone: 'info', title: `${key} was canceled`, description: launch.statusReason ?? undefined }
  }
  const count = (state: Launch['state']) => launches.filter((launch) => launch.state === state).length
  const failed = count('failed')
  const parts = [
    count('started') ? `${count('started')} started` : '',
    count('handed_off') ? `${count('handed_off')} handed off` : '',
    count('queued') ? `${count('queued')} queued` : '',
    count('cancelled') ? `${count('cancelled')} canceled` : '',
  ].filter(Boolean)
  const firstReason = launches.find((launch) => launch.state === 'failed' && launch.statusReason)?.statusReason ?? undefined
  if (failed === launches.length) return { tone: 'danger', title: `None of the ${launches.length} tasks started`, description: firstReason }
  if (failed) {
    return {
      tone: 'danger',
      title: `${countLabel(failed, 'task')} of ${launches.length} didn’t start`,
      description: [parts.length ? `The rest: ${parts.join(', ')}.` : '', firstReason ?? ''].filter(Boolean).join(' ') || undefined,
    }
  }
  return { tone: 'success', title: `${countLabel(launches.length, 'task')} launched`, description: parts.length ? `${parts.join(', ')}.`.replace(/^./, (first) => first.toUpperCase()) : undefined }
}
