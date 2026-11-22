import { callOp, type OpOutput } from '../api/ops'

/**
 * A run's state. The run record's states (A1) first; the built-in host's
 * own states after them, still sent by its run page and kept for cached data.
 */
export type RunState =
  | 'queued'
  | 'starting'
  | 'start_uncertain'
  | 'accepted'
  | 'working'
  | 'waiting'
  | 'cancel_requested'
  | 'done'
  | 'failed'
  | 'cancelled'
  | 'refused'
  | 'interrupted'
  | 'created'
  | 'blocked'
  | 'recovering'
  | 'completed'
  | 'canceled'

export type Liveness = 'ok' | 'quiet' | 'not_responding'

export type CostConfidence = 'exact' | 'estimated'

export type RunStatusFilter = 'all' | 'active' | 'needs_you' | 'not_responding' | 'finished' | 'failed'

export type UsageTotals = {
  inputTokens: number
  outputTokens: number
  cachedInputTokens: number
  /** Null when neither the catalog nor the harness knows the price. */
  cost: number | null
  currency: string | null
}

export type RunSummary = UsageTotals & {
  id: string
  state: RunState
  organizationId: string
  projectId: string
  projectKey: string
  projectName: string
  conversationId: string | null
  threadName: string | null
  taskId: string | null
  taskKey: string | null
  taskTitle: string | null
  harness: string | null
  harnessName: string | null
  model: string | null
  reasoning: string | null
  speed: string | null
  requestedByActorId: string | null
  requestedByName: string | null
  agentActorId: string | null
  agentName: string | null
  agentProfileId: string | null
  branch: string | null
  baseCommit: string | null
  publishedCommit: string | null
  pullRequestNumber: number | null
  pullRequestUrl: string | null
  pullRequestState: string | null
  failureReason: string | null
  createdAt: string
  startedAt: string | null
  finishedAt: string | null
  updatedAt: string
}

/** A run as the Runs page lists it: any run, whoever runs it. */
export type RunLine = UsageTotals & {
  id: string
  state: RunState
  liveness: Liveness
  title: string
  causeKind: string
  causeRef: string | null
  organizationId: string
  projectId: string | null
  projectKey: string | null
  projectName: string | null
  conversationId: string | null
  threadName: string | null
  taskId: string | null
  taskKey: string | null
  taskTitle: string | null
  agentActorId: string
  agentName: string
  /** None when the agent reports for itself. */
  hostId: string | null
  hostName: string | null
  backend: string | null
  /** This server's own host runs it. */
  builtin: boolean
  harness: string | null
  harnessName: string | null
  model: string | null
  requestedByActorId: string | null
  requestedByName: string | null
  waitingOnRequestId: string | null
  waitingOnTitle: string | null
  statusLine: string | null
  outcomeSummary: string | null
  externalUrl: string | null
  failureReason: string | null
  branch: string | null
  pullRequestNumber: number | null
  pullRequestUrl: string | null
  pullRequestState: string | null
  gapCount: number
  createdAt: string
  startedAt: string | null
  finishedAt: string | null
  updatedAt: string
  lastReportAt: string | null
  costConfidence: CostConfidence | null
}

export type RunList = { runs: RunLine[]; nextCursor: string | null }

/** One thing a run reported doing, as the run view draws it. */
export type RunActivity = {
  seq: number
  kind: string
  stepRef: string | null
  parentRef: string | null
  summary: string
  payload: Record<string, unknown>
  occurredAt: string
}

/** A request the run raised: an approval, question, input or proposal. */
export type RunRequest = {
  id: string
  kind: string
  title: string
  /** open, answered, expired, cancelled or superseded. */
  status: string
  conversationId: string | null
  createdAt: string
  answeredAt: string | null
}

export type PlanItem = { title: string; status?: string }

/** What a run produced: a pull request, branch, diff, thing, file or link. */
export type RunResult = {
  kind: string
  url?: string
  id?: string
  branch?: string
  title?: string
  thingKind?: string
  number?: number
}

export type ModelUsageLine = UsageTotals & { model: string; confidence: CostConfidence }

/** Any run with what it did: the run page. */
export type RunView = RunLine & {
  plan: PlanItem[] | null
  activities: RunActivity[]
  hiddenActivities: number
  requests: RunRequest[]
  results: RunResult[]
  /** Pull requests it opened or reported that the space has, with their checks. */
  pullRequests: OpOutput<'run_detail_get'>['pullRequests']
  usageByModel: ModelUsageLine[]
  /** What it changed, as its host last reported it; null before a report (or without "see code"). */
  files: ChangedFile[] | null
  /** The host couldn't report all of its changes, so `files` may be missing some. */
  changesTruncated: boolean
}

export type StepFile = { path: string; change?: string }

export type StepPayload = {
  tool?: string
  command?: string
  exitCode?: number | null
  output?: string
  files?: StepFile[]
  tests?: { passed?: number; failed?: number; skipped?: number }
  failed?: boolean
  text?: string
  truncated?: boolean
  [key: string]: unknown
}

export type RunStep = {
  id: string
  kind: string
  summary: string
  payload: StepPayload | null
  createdAt: string
}

export type TestSummary = { runs: number; passed: number; failed: number; skipped: number; lastFailed: number | null }

export type ModelUsage = UsageTotals & { model: string }

export type ChangedFile = {
  path: string
  /** added, modified, deleted or renamed. */
  status: string
  /** Null for binary files. */
  added: number | null
  removed: number | null
}

export type RunDetail = RunSummary & {
  prompt: string
  steps: RunStep[]
  hiddenSteps: number
  tests: TestSummary
  usageByModel: ModelUsage[]
  files: ChangedFile[] | null
}

export type RunDiff = {
  state: 'ready' | 'unavailable'
  files?: ChangedFile[]
  /** Empty when the diff is too big to show. */
  patch?: string
  truncated?: boolean
}

export type UsageGroup = 'day' | 'project' | 'account' | 'model'

export type UsageRow = UsageTotals & { key: string; label: string; runs: number }

export type UsageReport = { groupBy: UsageGroup; since: string; rows: UsageRow[]; totals: UsageTotals; runs: number }

export type RunListFilter = {
  /** The workspace to list, when no project names one. */
  workspaceId?: string
  projectId?: string
  status?: RunStatusFilter
  agentId?: string
  hostId?: string
  harness?: string
  mine?: boolean
}

export type UsageFilter = { workspaceId?: string; projectId?: string; groupBy: UsageGroup; days?: number }

export const RUN_PAGE_SIZE = 50

/** Runs cross projects, so their keys sit outside any one project's. */
export { runKeys } from './keys'

export function listRuns(filter: RunListFilter, cursor?: string, signal?: AbortSignal) {
  return callOp(
    'run_line_list',
    {
      spaceId: filter.projectId,
      status: filter.status === 'all' ? undefined : filter.status,
      agentId: filter.agentId,
      hostId: filter.hostId,
      harness: filter.harness,
      mine: filter.mine || undefined,
      cursor,
      limit: RUN_PAGE_SIZE,
    },
    { signal, workspaceId: filter.workspaceId },
  ).then((value) => value as unknown as RunList)
}

export function viewRun(runId: string, signal?: AbortSignal) {
  return callOp('run_detail_get', { runId }, { signal, retries: 0 }).then((value) => value as unknown as RunView)
}

export function listRunActivities(runId: string, afterSeq?: number) {
  return callOp('run_activity_list', { runId, afterSeq }).then(
    (value) => value as unknown as { activities: RunActivity[]; nextAfterSeq: number | null },
  )
}

export function cancelRun(runId: string) {
  return callOp('run_cancel', { runId })
}

export function markRunNotStarted(runId: string) {
  return callOp('run_mark_not_started', { runId })
}

/** A run on this server's own host: its full record (steps, tests, files). */
export function getRun(runId: string, signal?: AbortSignal) {
  return callOp('run_step_list', { runId }, { signal }).then((value) => value.run as unknown as RunDetail)
}

export function getRunDiff(runId: string, signal?: AbortSignal) {
  return callOp('run_diff_get', { runId }, { signal }).then((value) => value as unknown as RunDiff)
}

export function getUsage(filter: UsageFilter, signal?: AbortSignal) {
  return callOp(
    'run_usage_list',
    { projectId: filter.projectId, groupBy: filter.groupBy, days: filter.days },
    { signal, workspaceId: filter.workspaceId },
  ).then(
    (value) => value.usage as unknown as UsageReport,
  )
}
