import { requestJson } from '../api/client'

export type RunState =
  | 'created'
  | 'queued'
  | 'starting'
  | 'working'
  | 'blocked'
  | 'recovering'
  | 'completed'
  | 'failed'
  | 'canceled'
  | 'interrupted'

export type RunStatusFilter = 'all' | 'active' | 'needs_you' | 'finished' | 'failed'

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

export type RunList = { runs: RunSummary[]; nextCursor: string | null }

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
  organizationId?: string
  projectId?: string
  status?: RunStatusFilter
  harness?: string
  mine?: boolean
}

export type UsageFilter = { organizationId?: string; projectId?: string; groupBy: UsageGroup; days?: number }

export const RUN_PAGE_SIZE = 50

/** Runs cross projects, so their keys sit outside any one project's. */
export const runKeys = {
  list: (filter: RunListFilter) => ['runs', 'list', filter] as const,
  usage: (filter: UsageFilter) => ['runs', 'usage', filter] as const,
  detail: (runId: string) => ['runs', 'detail', runId] as const,
  diff: (runId: string) => ['runs', 'detail', runId, 'diff'] as const,
}

function query(values: Record<string, string | number | boolean | undefined>) {
  const params = new URLSearchParams()
  for (const [key, value] of Object.entries(values)) {
    if (value === undefined || value === '' || value === false) continue
    params.set(key, String(value))
  }
  return params.size ? `?${params.toString()}` : ''
}

export function listRuns(filter: RunListFilter, cursor?: string) {
  return requestJson<RunList>(
    `/api/v1/runs${query({
      organizationId: filter.organizationId,
      projectId: filter.projectId,
      status: filter.status === 'all' ? undefined : filter.status,
      harness: filter.harness,
      mine: filter.mine,
      cursor,
      limit: RUN_PAGE_SIZE,
    })}`,
  )
}

export function getRun(runId: string) {
  return requestJson<RunDetail>(`/api/v1/agent-runs/${encodeURIComponent(runId)}`)
}

export function getRunDiff(runId: string) {
  return requestJson<RunDiff>(`/api/v1/agent-runs/${encodeURIComponent(runId)}/diff`)
}

export function getUsage(filter: UsageFilter) {
  return requestJson<UsageReport>(`/api/v1/runs/usage${query(filter)}`)
}
