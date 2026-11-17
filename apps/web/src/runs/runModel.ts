import { eventLabel, formatTime, type ChipTone, type IconName } from '../ui'
import type { CostConfidence, Liveness, RunActivity, RunLine, RunState, RunStep, RunSummary, UsageTotals } from './api'

export const LIVE_STATES: readonly RunState[] = [
  'queued',
  'starting',
  'start_uncertain',
  'accepted',
  'working',
  'waiting',
  'cancel_requested',
  // The built-in host's own.
  'created',
  'blocked',
  'recovering',
]

/** States where the run waits on someone's answer. */
export const WAITING_STATES: readonly RunState[] = ['waiting', 'blocked']

export function isLiveRun(state: RunState) {
  return LIVE_STATES.includes(state)
}

/**
 * How each run state reads, everywhere a run shows: the Runs list, the run
 * page, and (to be shared) the thread's run card and the agent sidebar.
 */
export const RUN_STATUS: Record<RunState, { label: string; tone: ChipTone; icon: IconName; spin?: boolean }> = {
  queued: { label: 'Queued', tone: 'muted', icon: 'clock' },
  starting: { label: 'Starting', tone: 'agent', icon: 'loader', spin: true },
  start_uncertain: { label: 'Start uncertain', tone: 'warning', icon: 'circle-help' },
  accepted: { label: 'Accepted', tone: 'agent', icon: 'loader', spin: true },
  working: { label: 'Working', tone: 'agent', icon: 'loader', spin: true },
  waiting: { label: 'Waiting on an answer', tone: 'warning', icon: 'circle-help' },
  cancel_requested: { label: 'Stopping…', tone: 'muted', icon: 'loader', spin: true },
  done: { label: 'Done', tone: 'success', icon: 'circle-check' },
  failed: { label: 'Failed', tone: 'danger', icon: 'circle-alert' },
  cancelled: { label: 'Stopped', tone: 'muted', icon: 'circle-minus' },
  refused: { label: 'Refused', tone: 'danger', icon: 'circle-minus' },
  interrupted: { label: 'Interrupted', tone: 'danger', icon: 'triangle-alert' },
  // The built-in host's own states.
  created: { label: 'Queued', tone: 'muted', icon: 'clock' },
  blocked: { label: 'Waiting on you', tone: 'warning', icon: 'circle-help' },
  recovering: { label: 'Recovering', tone: 'warning', icon: 'retry' },
  completed: { label: 'Done', tone: 'success', icon: 'circle-check' },
  canceled: { label: 'Stopped', tone: 'muted', icon: 'circle-minus' },
}

export function runStatus(state: string) {
  return RUN_STATUS[state as RunState] ?? { label: eventLabel(state), tone: 'muted' as ChipTone, icon: 'info' as IconName }
}

/** "12 min", "3 hr": how long since a time, for a badge. */
function since(time: string | null | undefined, now: number) {
  if (!time) return null
  const elapsed = now - new Date(time).getTime()
  return Number.isFinite(elapsed) && elapsed >= 0 ? formatDuration(elapsed) : null
}

/**
 * Whether a run is still reporting, in words, or null while it is (or once
 * it has finished, when it no longer has to).
 */
export function livenessLabel(
  run: { liveness: Liveness; state: RunState; lastReportAt: string | null; startedAt?: string | null },
  now = Date.now(),
): { label: string; tone: ChipTone } | null {
  if (run.liveness === 'ok' || !isLiveRun(run.state)) return null
  const quiet = since(run.lastReportAt ?? run.startedAt, now)
  if (run.liveness === 'not_responding') {
    return { label: quiet ? `Not responding for ${quiet}` : 'Not responding', tone: 'danger' }
  }
  return { label: quiet ? `Quiet for ${quiet}` : 'Quiet', tone: 'warning' }
}

/**
 * The cost as the Runs page shows it: exact, or "≈" with why when it was
 * shared out from a usage window. Null when nobody knows it.
 */
export function costText(cost: number | null | undefined, currency: string | null | undefined, confidence: CostConfidence | null | undefined) {
  const amount = formatCost(cost, currency)
  if (!amount) return null
  if (confidence === 'estimated') return { text: `≈ ${amount}`, hint: 'Estimated from usage windows' }
  return { text: amount, hint: null }
}

const CAUSES: Record<string, string> = {
  task: 'From a task',
  thread: 'From a thread',
  message: 'From a message',
  schedule: 'On a schedule',
  host_trigger: 'Started by its host',
  automation: 'From an automation',
  request: 'From a request',
  manual: 'Started by hand',
}

/** Why the run exists, in words. */
export function causeLabel(run: Pick<RunLine, 'causeKind' | 'causeRef' | 'taskKey'>) {
  if (run.causeKind === 'task' && run.taskKey) return run.taskKey
  if (run.causeKind === 'schedule' && run.causeRef) return `Scheduled: ${run.causeRef}`
  return CAUSES[run.causeKind] ?? eventLabel(run.causeKind)
}

/** Where the run runs: its host, this server, or the agent itself. */
export function hostLabel(run: Pick<RunLine, 'builtin' | 'hostName' | 'harnessName' | 'backend'>) {
  if (run.builtin) return run.harnessName ?? 'This server'
  if (!run.hostName) return 'Reports itself'
  return run.backend ? `${run.hostName} · ${run.backend}` : run.hostName
}

/** 950, 12.3k, 4.1M: short enough for a table cell. */
export function formatTokens(count: number) {
  if (!Number.isFinite(count) || count <= 0) return '0'
  if (count < 1000) return String(Math.round(count))
  if (count < 1_000_000) return `${trim(count / 1000)}k`
  return `${trim(count / 1_000_000)}M`
}

function trim(value: number) {
  return value >= 100 ? String(Math.round(value)) : value.toFixed(1).replace(/\.0$/, '')
}

export function totalTokens(usage: Pick<UsageTotals, 'inputTokens' | 'outputTokens'>) {
  return (usage.inputTokens || 0) + (usage.outputTokens || 0)
}

/**
 * The cost in its own currency, or null when it isn't known. A known zero
 * is "$0.00", not unknown; small amounts keep enough digits to read.
 */
export function formatCost(cost: number | null | undefined, currency: string | null | undefined) {
  if (cost == null || !Number.isFinite(cost)) return null
  const code = currency && /^[A-Za-z]{3}$/.test(currency) ? currency.toUpperCase() : 'USD'
  const digits = cost !== 0 && Math.abs(cost) < 0.01 ? 4 : 2
  try {
    return new Intl.NumberFormat(undefined, { style: 'currency', currency: code, minimumFractionDigits: 2, maximumFractionDigits: digits }).format(cost)
  } catch {
    return `${cost.toFixed(digits)} ${code}`
  }
}

export type StepView = {
  icon: IconName
  tone: 'muted' | 'agent' | 'warning' | 'success' | 'danger'
  title: string
}

/** How one step reads on the timeline. Unknown kinds fall back to their summary. */
export function stepView(step: RunStep): StepView {
  const payload = step.payload ?? {}
  const title = step.summary || step.kind.replaceAll('_', ' ')
  switch (step.kind) {
    case 'command_started':
    case 'command_completed': {
      const code = typeof payload.exitCode === 'number' ? payload.exitCode : null
      const tone = code === null ? 'muted' : code === 0 ? 'success' : 'danger'
      return { icon: 'terminal', tone, title: payload.command || title }
    }
    case 'file_change':
      return { icon: 'pencil', tone: 'muted', title }
    case 'approval_requested':
    case 'question_requested':
      return { icon: 'circle-help', tone: 'warning', title }
    case 'approval_resolved':
    case 'question_resolved':
      return { icon: 'check', tone: 'muted', title }
    case 'test_result': {
      const failed = (payload.tests?.failed ?? 0) > 0 || payload.failed === true
      return { icon: 'checklist', tone: failed ? 'danger' : 'success', title }
    }
    case 'agent_note':
      return { icon: 'message', tone: 'agent', title: payload.text || title }
    case 'tool_used':
      return { icon: 'activity', tone: 'muted', title: payload.tool ? `${payload.tool}: ${title}` : title }
    case 'run_completed':
      return { icon: 'circle-check', tone: 'success', title }
    case 'run_failed':
    case 'run_interrupted':
      return { icon: 'circle-alert', tone: 'danger', title }
    case 'recovery':
      return { icon: 'retry', tone: 'warning', title }
    default:
      return { icon: 'info', tone: 'muted', title }
  }
}

/**
 * How one reported activity reads on the run's timeline. A gap is a stretch
 * the run didn't report, so it stands out.
 */
export function activityView(activity: RunActivity): StepView {
  const payload = activity.payload ?? {}
  const text = (key: string) => (typeof payload[key] === 'string' ? (payload[key] as string) : null)
  const title = activity.summary || eventLabel(activity.kind)
  switch (activity.kind) {
    case 'run_started':
      return { icon: 'play', tone: 'muted', title }
    case 'run_finished': {
      const outcome = text('outcome')
      if (outcome === 'failed') return { icon: 'circle-alert', tone: 'danger', title }
      if (outcome === 'cancelled' || outcome === 'canceled') return { icon: 'circle-minus', tone: 'muted', title }
      return { icon: 'circle-check', tone: 'success', title }
    }
    case 'run_error':
      return { icon: 'circle-alert', tone: 'danger', title }
    case 'step_started':
    case 'step_finished':
      return { icon: 'checklist', tone: 'muted', title }
    case 'text_message':
      return { icon: 'message', tone: 'agent', title }
    case 'reasoning':
      return { icon: 'info', tone: 'muted', title }
    case 'tool_call':
      return { icon: 'terminal', tone: 'muted', title }
    case 'plan':
      return { icon: 'list', tone: 'muted', title }
    case 'interrupt':
      return { icon: 'circle-help', tone: 'warning', title }
    case 'interrupt_resolved':
      return { icon: 'check', tone: 'muted', title }
    case 'result':
      return { icon: 'link', tone: 'success', title }
    case 'gap':
      return { icon: 'triangle-alert', tone: 'warning', title }
    default:
      return { icon: 'info', tone: 'muted', title }
  }
}

const PLAN_STATUS: Record<string, { icon: IconName; label: string }> = {
  done: { icon: 'circle-check', label: 'Done' },
  completed: { icon: 'circle-check', label: 'Done' },
  in_progress: { icon: 'loader', label: 'In progress' },
  working: { icon: 'loader', label: 'In progress' },
  skipped: { icon: 'circle-minus', label: 'Skipped' },
  failed: { icon: 'circle-alert', label: 'Failed' },
}

/** One plan item's mark: done, in progress, skipped, failed, or still to do. */
export function planMark(status: string | undefined) {
  return PLAN_STATUS[status ?? ''] ?? { icon: 'clock' as IconName, label: 'To do' }
}

/** "40 sec", "14 min", "1 hr 5 min", "2 days 3 hr": how long a run took. */
export function formatDuration(milliseconds: number) {
  if (!Number.isFinite(milliseconds) || milliseconds < 0) return ''
  const seconds = Math.round(milliseconds / 1000)
  if (seconds < 60) return `${seconds} sec`
  const minutes = Math.round(seconds / 60)
  if (minutes < 60) return `${minutes} min`
  const hours = Math.floor(minutes / 60)
  if (hours < 24) return minutes % 60 ? `${hours} hr ${minutes % 60} min` : `${hours} hr`
  const days = Math.floor(hours / 24)
  return `${days} ${days === 1 ? 'day' : 'days'}${hours % 24 ? ` ${hours % 24} hr` : ''}`
}

/** How long the run has taken so far, or took. Empty before it starts. */
export function runDuration(run: { startedAt: string | null; finishedAt: string | null }, now = Date.now()) {
  if (!run.startedAt) return ''
  const end = run.finishedAt ? new Date(run.finishedAt).getTime() : now
  return formatDuration(end - new Date(run.startedAt).getTime())
}

/** The whole run as plain text, one block per step, for reading or copying. */
export function logText(steps: RunStep[]) {
  return steps
    .map((step) => {
      const payload = step.payload ?? {}
      const lines = [`${formatTime(step.createdAt)}  ${eventLabel(step.kind)}${step.summary ? `: ${step.summary}` : ''}`]
      if (payload.command) lines.push(`$ ${payload.command}`)
      if (typeof payload.exitCode === 'number') lines.push(`exit ${payload.exitCode}`)
      if (payload.files?.length) lines.push(...payload.files.map((file) => `  ${file.change ?? 'changed'} ${file.path}`))
      if (payload.text && payload.text !== step.summary) lines.push(payload.text)
      if (payload.output) lines.push(payload.output.replace(/\n+$/, ''))
      if (payload.truncated) lines.push('(output cut short)')
      return lines.join('\n')
    })
    .join('\n\n')
}

export type PatchFile = { path: string; text: string }

/**
 * Splits a unified diff into one piece per file, named by its new path (the
 * old one for a deletion), so each file can be opened on its own.
 */
export function splitPatch(patch: string): PatchFile[] {
  if (!patch.trim()) return []
  const files: PatchFile[] = []
  const chunks = patch.split(/^(?=diff --git )/m)
  for (const chunk of chunks) {
    if (!chunk.startsWith('diff --git ')) continue
    const next = /^\+\+\+ (?:b\/)?(.+)$/m.exec(chunk)?.[1]
    const old = /^--- (?:a\/)?(.+)$/m.exec(chunk)?.[1]
    const header = /^diff --git a\/(.+?) b\/(.+)$/m.exec(chunk)
    const path = next && next !== '/dev/null' ? next : old && old !== '/dev/null' ? old : (header?.[2] ?? header?.[1] ?? 'file')
    files.push({ path: path.trim(), text: chunk.replace(/\n+$/, '') })
  }
  return files
}

export function runTitle(run: Pick<RunSummary, 'taskKey' | 'taskTitle' | 'threadName'> & { title?: string | null }) {
  if (run.taskKey && run.taskTitle) return `${run.taskKey} ${run.taskTitle}`
  return run.taskTitle ?? run.threadName ?? run.title ?? 'Run'
}
