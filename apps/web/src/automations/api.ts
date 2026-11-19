import { requestJson } from '../api/client'
import { callOp, type OpInput, type OpOutput } from '../api/ops'

/**
 * Automation rules and the public API's document (W14). A person's
 * webhooks are connections (settings/account/WebhooksPage, on callOp).
 * The query keys live here so the pages that use them stay out of the first
 * load; project keys start with the project so a project-wide invalidation
 * reaches them.
 */
export const automationKeys = {
  rules: (projectId: string) => ['project', projectId, 'automations'] as const,
  channels: (projectId: string) => ['project', projectId, 'automations', 'channels'] as const,
  runs: (projectId: string, ruleId: string) => ['project', projectId, 'automations', ruleId, 'runs'] as const,
  openapi: ['public-api', 'openapi'] as const,
  operations: ['api', 'v2', 'openapi'] as const,
  preview: (projectId: string, ruleId: string, taskId: string | null) => ['project', projectId, 'automations', ruleId, 'preview', taskId] as const,
}

// Automation rules

export type Trigger =
  | { kind: 'task_created' }
  | { kind: 'task_matches' }
  | { kind: 'run_finished'; outcomes?: string[] }
  | { kind: 'pull_request_checks_failed' }
  | {
      kind: 'message_posted'
      conversationId?: string | null
      contains?: string | null
    }
  /** A collection's records (AWD-110): one collection, or any. */
  | { kind: 'record_created'; collectionId?: string | null }
  | { kind: 'record_changed'; collectionId?: string | null }
  /** A page, sheet or board kept as a new version: one of them, or any in the project. */
  | { kind: 'page_changed'; pageId?: string | null }
  | { kind: 'sheet_changed'; sheetId?: string | null }
  | { kind: 'whiteboard_changed'; whiteboardId?: string | null }
  | {
      kind: 'schedule'
      frequency: 'daily' | 'weekly' | 'monthly'
      every: number
      weekdays?: number[]
      monthDay?: number | null
      time: string
      timezone: string
    }
  | {
      kind: 'capacity_window'
      providerAccountId?: string | null
      maxTasks?: number | null
    }

export type TriggerKind = Trigger['kind']

export type Conditions = {
  viewId?: string | null
  filters?: Record<string, string>
}

export type Recipients = 'owner' | 'assignee' | 'project_admins'

export type RuleAction =
  | { kind: 'launch'; environmentId?: string | null; prompt?: string | null; agentProfileId?: string | null }
  | {
      kind: 'set_field'
      field: 'status' | 'priority' | 'assignee' | 'add_label' | 'remove_label'
      value: string
    }
  | { kind: 'notify'; recipients: Recipients; message: string }
  | { kind: 'post_message'; conversationId: string; text: string }
  | { kind: 'run_operation'; operation: string; input: Record<string, unknown> }
  | { kind: 'ask_agent'; accountId: string; prompt: string; conversationId?: string | null }

export type RuleActionKind = RuleAction['kind']

export type AutomationRule = {
  id: string
  projectId: string
  name: string
  enabled: boolean
  trigger: Trigger
  conditions: Conditions
  viewName: string | null
  actions: RuleAction[]
  ownerActorId: string
  ownerName: string | null
  viaApp: boolean
  nextRunAt: string | null
  disabledReason: string | null
  lastFiredAt: string | null
  fireCount: number
  version: number
  canEdit: boolean
  canManage: boolean
  createdAt: string
  updatedAt: string
}

export type RuleDraft = {
  name: string
  trigger: Trigger
  conditions: Conditions
  actions: RuleAction[]
  enabled?: boolean
}

export type AutomationRun = {
  id: string
  triggerSummary: string
  subject: Record<string, unknown>
  state: 'pending' | 'succeeded' | 'partial' | 'failed' | 'skipped'
  detail: string | null
  results: { kind: RuleActionKind; ok: boolean; detail: string }[]
  createdAt: string
  finishedAt: string | null
}

// Every rule call is an operation; their answers are the rule views the
// server has always sent, typed here.
type OpTrigger = OpInput<'automation_create'>['trigger']
type OpActions = OpInput<'automation_create'>['actions']
const asInput = (draft: Partial<RuleDraft>) => ({
  ...draft,
  ...(draft.trigger ? { trigger: draft.trigger as unknown as OpTrigger } : {}),
  ...(draft.actions ? { actions: draft.actions as unknown as OpActions } : {}),
})

export const listRules = async (projectId: string) =>
  (await callOp('automation_list', { projectId })) as { rules: AutomationRule[]; canCreate: boolean }

export const createRule = async (projectId: string, draft: RuleDraft) =>
  (await callOp('automation_create', { projectId, ...asInput(draft) } as OpInput<'automation_create'>)) as AutomationRule

export const updateRule = async (projectId: string, rule: Pick<AutomationRule, 'id' | 'version'>, change: Partial<RuleDraft>) =>
  (await callOp('automation_update', { projectId, ruleId: rule.id, expectedVersion: rule.version, ...asInput(change) })) as AutomationRule

export const deleteRule = async (projectId: string, ruleId: string) => {
  await callOp('automation_delete', { projectId, ruleId })
}

export const runRule = async (projectId: string, ruleId: string, workItemId?: string) =>
  (await callOp('automation_run_start', { projectId, ruleId, ...(workItemId ? { taskId: workItemId } : {}) })) as AutomationRun

/** What a rule would do if it fired now, worked out without doing any of it. */
export type RulePreview = OpOutput<'automation_preview'>

export const previewRule = (projectId: string, ruleId: string, workItemId?: string) =>
  callOp('automation_preview', { projectId, ruleId, ...(workItemId ? { taskId: workItemId } : {}) })

export const listRuns = async (projectId: string, ruleId: string) =>
  (await callOp('automation_history', { projectId, ruleId })) as { runs: AutomationRun[] }

// The public API's OpenAPI document

export type OpenApiSchema = {
  type?: string | string[]
  description?: string
  properties?: Record<string, OpenApiSchema>
  required?: string[]
  items?: OpenApiSchema
  enum?: unknown[]
  $ref?: string
  oneOf?: OpenApiSchema[]
  anyOf?: OpenApiSchema[]
  allOf?: OpenApiSchema[]
  format?: string
  const?: unknown
}

export type OpenApiOperation = {
  operationId: string
  summary?: string
  description?: string
  tags?: string[]
  requestBody?: {
    content?: { 'application/json'?: { schema?: OpenApiSchema } }
  }
  'x-read-only'?: boolean
}

export type OpenApiDocument = {
  info: { title: string; version: string; description?: string }
  servers?: { url: string }[]
  paths: Record<string, { post?: OpenApiOperation }>
  components?: { schemas?: Record<string, OpenApiSchema> }
}

export const getOpenApi = () => requestJson<OpenApiDocument>('/api/public/v1/openapi.json')

/** Every operation's input schema, as the v2 document has them: the automation builder's forms. */
export const getOperationDocument = () => requestJson<OpenApiDocument>('/api/v2/openapi.json')
