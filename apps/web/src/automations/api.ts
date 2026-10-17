import { requestJson } from '../api/client'

/**
 * Outgoing webhooks, automation rules and the public API's document (W14).
 * The query keys live here so the pages that use them stay out of the first
 * load; project keys start with the project so a project-wide invalidation
 * reaches them.
 */
export const automationKeys = {
  webhooks: ['account', 'webhooks'] as const,
  deliveries: (webhookId: string) => ['account', 'webhooks', webhookId, 'deliveries'] as const,
  rules: (projectId: string) => ['project', projectId, 'automations'] as const,
  runs: (projectId: string, ruleId: string) => ['project', projectId, 'automations', ruleId, 'runs'] as const,
  openapi: ['public-api', 'openapi'] as const,
}

// Webhooks

export type WebhookState = 'active' | 'paused' | 'disabled'

export type Webhook = {
  id: string
  organizationId: string
  projectId: string | null
  projectName: string | null
  name: string
  url: string
  eventTypes: string[]
  state: WebhookState
  disabledReason: string | null
  consecutiveFailures: number
  lastDeliveryAt: string | null
  hasSecret: boolean
  viaApp: boolean
  version: number
  createdAt: string
  updatedAt: string
}

export type WebhookList = {
  webhooks: Webhook[]
  eventTypes: { name: string; description: string }[]
  projects: { id: string; name: string; organizationId: string }[]
  signing: string
}

export type WebhookDelivery = {
  id: string
  eventType: string
  projectId: string | null
  subject: Record<string, unknown>
  state: 'pending' | 'delivered' | 'failed' | 'skipped'
  attempts: number
  lastStatusCode: number | null
  lastError: string | null
  durationMs: number | null
  occurredAt: string
  createdAt: string
  deliveredAt: string | null
}

export type WebhookInput = { name: string; url: string; eventTypes: string[]; projectId?: string | null }

const json = <T>(path: string, method: string, body: unknown = {}) =>
  requestJson<T>(path, { method, json: body, idempotencyKey: crypto.randomUUID() })

export const listWebhooks = () => requestJson<WebhookList>('/api/v1/webhooks')

export const createWebhook = (input: WebhookInput) =>
  json<{ webhook: Webhook; secret: string }>('/api/v1/webhooks', 'POST', {
    name: input.name,
    url: input.url,
    eventTypes: input.eventTypes,
    ...(input.projectId ? { projectId: input.projectId } : {}),
  })

export const updateWebhook = (webhook: Pick<Webhook, 'id' | 'version'>, change: Partial<Pick<Webhook, 'name' | 'url' | 'eventTypes'>> & { state?: 'active' | 'paused' }) =>
  json<Webhook>(`/api/v1/webhooks/${encodeURIComponent(webhook.id)}`, 'PATCH', { expectedVersion: webhook.version, ...change })

export const deleteWebhook = (webhookId: string) =>
  requestJson<void>(`/api/v1/webhooks/${encodeURIComponent(webhookId)}`, { method: 'DELETE' })

export const rotateWebhookSecret = (webhookId: string) =>
  json<{ webhook: Webhook; secret: string }>(`/api/v1/webhooks/${encodeURIComponent(webhookId)}/rotate-secret`, 'POST')

export const testWebhook = (webhookId: string) =>
  json<WebhookDelivery>(`/api/v1/webhooks/${encodeURIComponent(webhookId)}/test`, 'POST')

export const listDeliveries = (webhookId: string) =>
  requestJson<{ deliveries: WebhookDelivery[]; nextCursor: string | null }>(
    `/api/v1/webhooks/${encodeURIComponent(webhookId)}/deliveries`,
  )

export const redeliver = (webhookId: string, deliveryId: string) =>
  json<WebhookDelivery>(
    `/api/v1/webhooks/${encodeURIComponent(webhookId)}/deliveries/${encodeURIComponent(deliveryId)}/redeliver`,
    'POST',
  )

// Automation rules

export type Trigger =
  | { kind: 'task_created' }
  | { kind: 'task_matches' }
  | { kind: 'run_finished'; outcomes?: string[] }
  | { kind: 'pull_request_checks_failed' }
  | { kind: 'message_posted'; conversationId?: string | null; contains?: string | null }
  | {
      kind: 'schedule'
      frequency: 'daily' | 'weekly' | 'monthly'
      every: number
      weekdays?: number[]
      monthDay?: number | null
      time: string
      timezone: string
    }
  | { kind: 'capacity_window'; providerAccountId?: string | null; maxTasks?: number | null }

export type TriggerKind = Trigger['kind']

export type Conditions = { viewId?: string | null; filters?: Record<string, string> }

export type Recipients = 'owner' | 'assignee' | 'project_admins'

export type RuleAction =
  | { kind: 'launch'; environmentId?: string | null; prompt?: string | null }
  | { kind: 'set_field'; field: 'status' | 'priority' | 'assignee' | 'add_label' | 'remove_label'; value: string }
  | { kind: 'notify'; recipients: Recipients; message: string }
  | { kind: 'post_message'; conversationId: string; text: string }

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

const rulesPath = (projectId: string, suffix = '') =>
  `/api/v1/projects/${encodeURIComponent(projectId)}/automations${suffix}`

export const listRules = (projectId: string) =>
  requestJson<{ rules: AutomationRule[]; canCreate: boolean }>(rulesPath(projectId))

export const createRule = (projectId: string, draft: RuleDraft) => json<AutomationRule>(rulesPath(projectId), 'POST', draft)

export const updateRule = (projectId: string, rule: Pick<AutomationRule, 'id' | 'version'>, change: Partial<RuleDraft>) =>
  json<AutomationRule>(rulesPath(projectId, `/${encodeURIComponent(rule.id)}`), 'PATCH', {
    expectedVersion: rule.version,
    ...change,
  })

export const deleteRule = (projectId: string, ruleId: string) =>
  requestJson<void>(rulesPath(projectId, `/${encodeURIComponent(ruleId)}`), { method: 'DELETE' })

export const runRule = (projectId: string, ruleId: string, workItemId?: string) =>
  json<AutomationRun>(rulesPath(projectId, `/${encodeURIComponent(ruleId)}/run`), 'POST', workItemId ? { workItemId } : {})

export const listRuns = (projectId: string, ruleId: string) =>
  requestJson<{ runs: AutomationRun[] }>(rulesPath(projectId, `/${encodeURIComponent(ruleId)}/runs`))

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
  requestBody?: { content?: { 'application/json'?: { schema?: OpenApiSchema } } }
  'x-read-only'?: boolean
}

export type OpenApiDocument = {
  info: { title: string; version: string; description?: string }
  servers?: { url: string }[]
  paths: Record<string, { post?: OpenApiOperation }>
  components?: { schemas?: Record<string, OpenApiSchema> }
}

export const getOpenApi = () => requestJson<OpenApiDocument>('/api/public/v1/openapi.json')
