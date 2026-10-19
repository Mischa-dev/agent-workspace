import { eventLabel, formatTime, type ChipTone, type IconName } from '../ui'
import type { RunState, RunStep, RunSummary, UsageTotals } from './api'

export const LIVE_STATES: readonly RunState[] = ['created', 'queued', 'starting', 'working', 'blocked', 'recovering']

export function isLiveRun(state: RunState) {
  return LIVE_STATES.includes(state)
}

/**
 * How each run state reads, everywhere a run shows: the Runs list, the run
 * page, and (to be shared) the thread's run card and the agent sidebar.
 */
export const RUN_STATUS: Record<RunState, { label: string; tone: ChipTone; icon: IconName; spin?: boolean }> = {
  created: { label: 'Queued', tone: 'muted', icon: 'clock' },
  queued: { label: 'Queued', tone: 'muted', icon: 'clock' },
  starting: { label: 'Starting', tone: 'agent', icon: 'loader', spin: true },
  working: { label: 'Working', tone: 'agent', icon: 'loader', spin: true },
  blocked: { label: 'Waiting on you', tone: 'warning', icon: 'circle-help' },
  recovering: { label: 'Recovering', tone: 'warning', icon: 'retry' },
  completed: { label: 'Done', tone: 'success', icon: 'circle-check' },
  failed: { label: 'Failed', tone: 'danger', icon: 'circle-alert' },
  canceled: { label: 'Stopped', tone: 'muted', icon: 'circle-minus' },
  interrupted: { label: 'Interrupted', tone: 'danger', icon: 'triangle-alert' },
}

export function runStatus(state: string) {
  return RUN_STATUS[state as RunState] ?? { label: eventLabel(state), tone: 'muted' as ChipTone, icon: 'info' as IconName }
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
export function runDuration(run: Pick<RunSummary, 'startedAt' | 'finishedAt'>, now = Date.now()) {
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

export function runTitle(run: Pick<RunSummary, 'taskKey' | 'taskTitle' | 'threadName'>) {
  if (run.taskKey && run.taskTitle) return `${run.taskKey} ${run.taskTitle}`
  return run.taskTitle ?? run.threadName ?? 'Run'
}
