// AI planning (roadmap W10): the model gateway's status, suggestions to
// review, readiness and what to do next, the dependency graph, summaries and
// plan imports. Every route checks the reader's access on the server; nothing
// here decides what someone may see.
import { requestJson } from '../api/client'

export type SuggestionKind =
  | 'blocks'
  | 'relates_to'
  | 'duplicates'
  | 'parent'
  | 'blocked_reason'
  | 'label'
  | 'priority'
  | 'type'
  | 'criteria'

export type SuggestionStatus = 'pending' | 'accepted' | 'dismissed' | 'applied' | 'stale'

export type TaskRef = {
  id: string
  projectId: string
  key: string
  title: string
  statusName: string
  statusCategory: string
  finished: boolean
  archived: boolean
  href: string
}

export type SuggestionSource =
  | { kind: 'task'; task: TaskRef }
  | { kind: 'message'; messageId: string; conversationId: string; conversationName: string; href: string }
  | { kind: 'signal'; name: string }

export type Suggestion = {
  id: string
  kind: SuggestionKind
  task: TaskRef
  other: TaskRef | null
  value: Record<string, unknown>
  confidence: number
  reason: string
  sources: SuggestionSource[]
  status: SuggestionStatus
  origin: string
  /** Only the reader can see it: it drew on something not everyone in the project can see. */
  personal: boolean
  provider: string
  model: string
  createdAt: string
  decidedAt: string | null
}

export type SuggestionPage = {
  suggestions: Suggestion[]
  nextCursor: string | null
  pendingCount: number
  canEdit: boolean
}

export type AiRun = {
  id: string
  kind: 'edit' | 'analyze' | 'nightly' | 'triage' | string
  state: 'queued' | 'running' | 'succeeded' | 'failed'
  tasksConsidered: number
  suggestionsCreated: number
  suggestionsApplied: number
  error: string | null
  createdAt: string
  finishedAt: string | null
  mine: boolean
}

export type ProjectAi = {
  organizationId: string
  configured: boolean
  problem: string | null
  provider: { id: string; displayName: string } | null
  quickModel: string | null
  deepModel: string | null
  keySource: 'project' | 'workspace' | null
  backgroundDetection: boolean
  autoApply: boolean
  autoApplyMinConfidence: number
  pendingSuggestions: number
  runs: AiRun[]
  canEdit: boolean
}

export type ModelEntry = { id: string; displayName: string; description?: string | null; hidden?: boolean }

export type ProviderManifest = {
  id: string
  displayName: string
  description: string
  needsKey: boolean
  needsBaseUrl: boolean
  defaultBaseUrl: string | null
  defaultQuickModel: string
  defaultDeepModel: string
  models: ModelEntry[]
  testingOnly: boolean
}

export type WorkspaceAi = {
  providers: ProviderManifest[]
  /** Which providers have a workspace key saved. Never the key. */
  workspaceKeys: Record<string, boolean>
  configured: boolean
  problem: string | null
  provider: string | null
  quickModel: string | null
  budgetUsd: number
  spentMicros: number
  usageByModel: { model: string; provider: string; calls: number; inputTokens: number; outputTokens: number; costMicros: number }[]
  usageByDay: { day: string; calls: number; costMicros: number }[]
}

export type ConnectionCheck = { ok: true; provider: string; model: string; latencyMs: number }

export type ReadinessCheck = { id: string; ok: boolean; label: string; forAgents: boolean }
export type Readiness = { ready: boolean; readyForAgents: boolean; checks: ReadinessCheck[] }

export type RankScope = 'ready' | 'agents' | 'all'

export type RankedTask = {
  id: string
  key: string
  title: string
  statusName: string
  statusCategory: string
  priority: string
  milestoneId: string | null
  milestoneName: string | null
  assigneeName: string | null
  claimed: boolean
  score: number
  unblocks: number
  reasons: string[]
  readiness: Readiness
  href: string
}

export type Ranking = { tasks: RankedTask[]; nextOffset: number | null; total: number }

export type GraphNode = {
  id: string
  projectId: string
  key: string
  title: string
  statusName: string
  statusCategory: string
  finished: boolean
  priority: string
  estimate: number | null
  milestoneId: string | null
  milestoneName: string | null
  assigneeName: string | null
  external: boolean
  ready: boolean
  critical: boolean
  href: string
}

export type GraphEdge = {
  from: string
  to: string
  suggestionId: string | null
  confidence: number | null
  reason: string | null
}

export type Graph = {
  nodes: GraphNode[]
  edges: GraphEdge[]
  frontier: string[]
  criticalPath: string[]
  truncated: boolean
  canEdit: boolean
}

export type DraftMilestone = { ref: string; name: string; description: string; targetDate: string | null }
export type DraftTask = {
  ref: string
  title: string
  description: string
  criteria: string[]
  labels: string[]
  priority: string
  milestone: string | null
  parent: string | null
  blockedBy: string[]
}
export type Draft = { milestones: DraftMilestone[]; tasks: DraftTask[] }

export type PlanState = 'drafting' | 'draft' | 'committing' | 'committed' | 'failed'

export type PlanImport = {
  id: string
  title: string
  sourceKind: 'text' | 'page' | 'structured'
  state: PlanState
  draft: Draft
  error: string | null
  version: number
  createdAt: string
  committedAt: string | null
  createdMilestones: number
  createdTasks: number
  createdDependencies: number
}

export type PlanSource =
  | { kind: 'text'; text: string }
  | { kind: 'page'; pageId: string }
  | { kind: 'structured'; plan: unknown }

export type Summary = {
  lines: string[]
  outcome: string | null
  status: string | null
  entries: number
  provider: string
  model: string
}

const project = (projectId: string) => `/api/v1/projects/${encodeURIComponent(projectId)}`
const ai = (projectId: string) => `${project(projectId)}/ai`

export const aiKeys = {
  root: (projectId: string) => ['project', projectId, 'ai'] as const,
  status: (projectId: string) => ['project', projectId, 'ai', 'status'] as const,
  suggestions: (projectId: string, filters: Record<string, unknown>) => ['project', projectId, 'ai', 'suggestions', filters] as const,
  ranking: (projectId: string, filters: Record<string, unknown>) => ['project', projectId, 'ai', 'ranking', filters] as const,
  graph: (projectId: string, filters: Record<string, unknown>) => ['project', projectId, 'ai', 'graph', filters] as const,
  plans: (projectId: string) => ['project', projectId, 'ai', 'plans'] as const,
  plan: (projectId: string, importId: string) => ['project', projectId, 'ai', 'plans', importId] as const,
  readiness: (projectId: string, workItemId: string) => ['project', projectId, 'ai', 'readiness', workItemId] as const,
  workspace: (organizationId: string) => ['organization', organizationId, 'ai'] as const,
}

function query(params: Record<string, string | number | boolean | null | undefined>) {
  const search = new URLSearchParams()
  for (const [name, value] of Object.entries(params)) {
    if (value !== null && value !== undefined && value !== '') search.set(name, String(value))
  }
  const text = search.toString()
  return text ? `?${text}` : ''
}

export const getProjectAi = (projectId: string) => requestJson<ProjectAi>(ai(projectId))

export const testProjectAi = (projectId: string) =>
  requestJson<ConnectionCheck>(`${ai(projectId)}/test`, { method: 'POST', json: {} })

export const analyzeProject = (projectId: string) =>
  requestJson<{ runId: string }>(`${ai(projectId)}/analyze`, { method: 'POST', json: {} })

export type SuggestionFilters = {
  status?: SuggestionStatus | 'all'
  workItemId?: string
  kind?: string
  cursor?: string | null
  limit?: number
}

export const listSuggestions = (projectId: string, filters: SuggestionFilters = {}) =>
  requestJson<SuggestionPage>(`${ai(projectId)}/suggestions${query(filters)}`)

export const decideSuggestion = (projectId: string, suggestionId: string, decision: 'accept' | 'dismiss') =>
  requestJson<{ suggestion: Suggestion | null }>(
    `${ai(projectId)}/suggestions/${encodeURIComponent(suggestionId)}/${decision}`,
    { method: 'POST', json: {} },
  )

export type RankFilters = { scope?: RankScope; milestoneId?: string | null; limit?: number; offset?: number }

export const getRanking = (projectId: string, filters: RankFilters = {}) =>
  requestJson<Ranking>(`${ai(projectId)}/ready${query(filters)}`)

export type GraphFilters = { milestoneId?: string | null; includeFinished?: boolean }

export const getGraph = (projectId: string, filters: GraphFilters = {}) =>
  requestJson<Graph>(`${ai(projectId)}/graph${query({ milestoneId: filters.milestoneId, includeFinished: filters.includeFinished || undefined })}`)

export const getReadiness = (projectId: string, workItemId: string) =>
  requestJson<Readiness>(`${project(projectId)}/work-items/${encodeURIComponent(workItemId)}/readiness`)

const taskAi = (projectId: string, workItemId: string, action: string) =>
  `${project(projectId)}/work-items/${encodeURIComponent(workItemId)}/ai/${action}`

export const triageTask = (projectId: string, workItemId: string) =>
  requestJson<{ runId: string }>(taskAi(projectId, workItemId, 'triage'), { method: 'POST', json: {} })

export const draftCriteria = (projectId: string, workItemId: string) =>
  requestJson<{ suggestion: Suggestion | null }>(taskAi(projectId, workItemId, 'criteria'), { method: 'POST', json: {} })

export const summarizeTask = (projectId: string, workItemId: string) =>
  requestJson<{ summary: Summary }>(taskAi(projectId, workItemId, 'summary'), { method: 'POST', json: {} })

export const summarizeConversation = (conversationId: string, since?: string | null) =>
  requestJson<{ summary: Summary }>(`/api/v1/conversations/${encodeURIComponent(conversationId)}/ai/summary`, {
    method: 'POST',
    json: since ? { since } : {},
  })

export const listPlans = (projectId: string) =>
  requestJson<{ plans: PlanImport[]; canEdit: boolean }>(`${ai(projectId)}/plans`)

export const getPlan = (projectId: string, importId: string) =>
  requestJson<{ plan: PlanImport; canEdit: boolean }>(`${ai(projectId)}/plans/${encodeURIComponent(importId)}`)

export const createPlan = (projectId: string, title: string, source: PlanSource) =>
  requestJson<{ plan: PlanImport }>(`${ai(projectId)}/plans`, { method: 'POST', json: { title, source } })

export const savePlan = (projectId: string, plan: PlanImport, draft: Draft, title?: string) =>
  requestJson<{ plan: PlanImport }>(`${ai(projectId)}/plans/${encodeURIComponent(plan.id)}`, {
    method: 'PUT',
    json: { expectedVersion: plan.version, draft, ...(title ? { title } : {}) },
  })

export const commitPlan = (projectId: string, plan: PlanImport) =>
  requestJson<{ plan: PlanImport }>(`${ai(projectId)}/plans/${encodeURIComponent(plan.id)}/commit`, {
    method: 'POST',
    json: { expectedVersion: plan.version },
  })

const workspace = (organizationId: string) => `/api/v1/organizations/${encodeURIComponent(organizationId)}/ai`

export const getWorkspaceAi = (organizationId: string) => requestJson<WorkspaceAi>(workspace(organizationId))

export const testWorkspaceAi = (organizationId: string) =>
  requestJson<ConnectionCheck>(`${workspace(organizationId)}/test`, { method: 'POST', json: {} })

/** The secret name a provider's API key is saved under. */
export const providerKeyName = (providerId: string) => `ai_key.${providerId}`
