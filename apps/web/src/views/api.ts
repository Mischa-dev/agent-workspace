import { apiUrl, requestJson } from '../api/client'

/**
 * Saved views, My work, the timeline, templates, imports and export. The
 * query keys live here rather than in api/queryKeys so the pages that use
 * them stay out of the first load; project keys start with the project so a
 * project-wide invalidation reaches them.
 */
export const viewKeys = {
  views: (projectId: string) => ['project', projectId, 'saved-views'] as const,
  myWork: ['my-work'] as const,
  timeline: (projectId: string, query: Record<string, string>) => ['project', projectId, 'work-items', 'timeline-chart', query] as const,
  templates: (projectId: string) => ['project', projectId, 'task-templates'] as const,
  imports: (projectId: string) => ['project', projectId, 'task-imports'] as const,
  taskImport: (projectId: string, importId: string) => ['project', projectId, 'task-imports', importId] as const,
  projectTemplates: (organizationId: string, includeArchived = false) =>
    ['organization', organizationId, 'project-templates', includeArchived ? 'all' : 'active'] as const,
}

const projectPath = (projectId: string, path: string) => `/api/v1/projects/${encodeURIComponent(projectId)}/${path}`

// Saved views

export type SavedView = {
  id: string
  name: string
  builtin: string | null
  shared: boolean
  query: Record<string, string>
  resolvedQuery: Record<string, string>
  layout: 'list' | 'board'
  fields: string[] | null
  pinned: boolean
  position: number
  canEdit: boolean
  canDelete: boolean
  version: number
  href: string
}

export type ViewDraft = {
  name: string
  shared: boolean
  query: Record<string, string>
  layout: 'list' | 'board'
  fields: string[] | null
}

export async function listViews(projectId: string): Promise<{ views: SavedView[]; canShare: boolean }> {
  const response = await requestJson<{ views?: unknown; canShare?: unknown } | null>(projectPath(projectId, 'views'))
  return { views: Array.isArray(response?.views) ? (response.views as SavedView[]) : [], canShare: response?.canShare === true }
}

export function createView(projectId: string, draft: ViewDraft, idempotencyKey: string) {
  return requestJson<SavedView>(projectPath(projectId, 'views'), { method: 'POST', idempotencyKey, json: draft })
}

export function updateView(
  projectId: string,
  view: Pick<SavedView, 'id' | 'version'>,
  changes: Partial<ViewDraft>,
  idempotencyKey: string,
) {
  return requestJson<SavedView>(projectPath(projectId, `views/${encodeURIComponent(view.id)}`), {
    method: 'PATCH',
    idempotencyKey,
    json: { expectedVersion: view.version, ...changes },
  })
}

export function deleteView(projectId: string, view: Pick<SavedView, 'id' | 'version'>, idempotencyKey: string) {
  return requestJson<unknown>(projectPath(projectId, `views/${encodeURIComponent(view.id)}`), {
    method: 'DELETE',
    idempotencyKey,
    json: { expectedVersion: view.version },
  })
}

/** Pins a view to your sidebar or takes it off; `position` places it among your pins. */
export function pinView(projectId: string, viewId: string, pinned: boolean, idempotencyKey: string, position?: number) {
  return requestJson<SavedView>(projectPath(projectId, `views/${encodeURIComponent(viewId)}/pin`), {
    method: 'POST',
    idempotencyKey,
    json: position === undefined ? { pinned } : { pinned, position },
  })
}

// My work

export type MyWorkTask = {
  id: string
  projectId: string
  projectKey: string
  projectName: string
  key: string
  title: string
  statusName: string
  statusCategory: string
  priority: string
  startDate: string | null
  dueDate: string | null
  overdue: boolean
  blocked: boolean
  milestoneName: string | null
  assigneeName: string | null
  claimedByName: string | null
  claimedByKind: string | null
  updatedAt: string
}

export type MyWorkSection = { items: MyWorkTask[]; total: number }
export type MyWork = { assigned: MyWorkSection; claimed: MyWorkSection; review: MyWorkSection }

export function getMyWork() {
  return requestJson<MyWork>('/api/v1/my-work')
}

// Timeline

export type TimelineTask = {
  id: string
  key: string
  title: string
  statusName: string
  statusCategory: string
  priority: string
  startDate: string | null
  dueDate: string | null
  estimate: number | null
  milestoneId: string | null
  parentId: string | null
  assigneeName: string | null
  blocked: boolean
  version: number
}

export type TimelineMilestone = {
  id: string
  name: string
  targetDate: string | null
  overdue: boolean
  progress: number
  version: number
}

export type TimelineLink = { blockingId: string; blockedId: string }

export type Timeline = {
  from: string
  to: string
  estimateUnit: string
  milestones: TimelineMilestone[]
  tasks: TimelineTask[]
  dependencies: TimelineLink[]
  suggestedDependencies: TimelineLink[]
  truncated: boolean
  undated: number
  canEdit: boolean
}

export function getTimeline(projectId: string, query: Record<string, string>) {
  const search = new URLSearchParams(query).toString()
  return requestJson<Timeline>(projectPath(projectId, `timeline${search ? `?${search}` : ''}`))
}

// Task templates and recurring tasks

export type TaskTemplate = {
  id: string
  name: string
  title: string
  description: string
  typeId: string | null
  typeName: string | null
  priority: string
  labels: string[]
  acceptanceCriteria: string[]
  subtasks: Array<{ title: string; description: string }>
  estimate: number | null
  archived: boolean
  version: number
}

export type TemplateFields = Omit<TaskTemplate, 'id' | 'typeName' | 'archived' | 'version'>

export type Recurring = {
  id: string
  templateId: string
  templateName: string
  frequency: 'daily' | 'weekly' | 'monthly'
  every: number
  weekdays: number[]
  monthDay: number | null
  timeOfDay: string
  timezone: string
  startsOn: string
  assigneeActorId: string | null
  assigneeName: string | null
  dueInDays: number | null
  enabled: boolean
  pausedReason: string | null
  nextRunAt: string | null
  lastRunAt: string | null
  lastWorkItemId: string | null
  lastWorkItemKey: string | null
  actsAs: string | null
  version: number
}

export type ScheduleDraft = {
  frequency: Recurring['frequency']
  every: number
  weekdays: number[]
  monthDay: number | null
  timeOfDay: string
  timezone: string
  dueInDays: number | null
}

export function listTemplates(projectId: string) {
  return requestJson<{ templates: TaskTemplate[]; recurring: Recurring[]; canEdit: boolean }>(
    projectPath(projectId, 'task-templates'),
  )
}

export function createTemplate(projectId: string, fields: TemplateFields, idempotencyKey: string) {
  return requestJson<TaskTemplate>(projectPath(projectId, 'task-templates'), { method: 'POST', idempotencyKey, json: fields })
}

export function updateTemplate(
  projectId: string,
  template: Pick<TaskTemplate, 'id' | 'version'>,
  changes: Partial<TemplateFields> & { archived?: boolean },
  idempotencyKey: string,
) {
  return requestJson<TaskTemplate>(projectPath(projectId, `task-templates/${encodeURIComponent(template.id)}`), {
    method: 'PATCH',
    idempotencyKey,
    json: { expectedVersion: template.version, ...changes },
  })
}

export function applyTemplate(projectId: string, templateId: string, options: { title?: string }, idempotencyKey: string) {
  return requestJson<{ workItemId: string; key: string; subtaskCount: number }>(
    projectPath(projectId, `task-templates/${encodeURIComponent(templateId)}/apply`),
    { method: 'POST', idempotencyKey, json: options },
  )
}

export function createRecurring(projectId: string, templateId: string, schedule: ScheduleDraft, idempotencyKey: string) {
  return requestJson<Recurring>(projectPath(projectId, 'recurring-tasks'), {
    method: 'POST',
    idempotencyKey,
    json: { templateId, ...schedule },
  })
}

export function updateRecurring(
  projectId: string,
  recurring: Pick<Recurring, 'id' | 'version'>,
  changes: Partial<ScheduleDraft> & { enabled?: boolean },
  idempotencyKey: string,
) {
  return requestJson<Recurring>(projectPath(projectId, `recurring-tasks/${encodeURIComponent(recurring.id)}`), {
    method: 'PATCH',
    idempotencyKey,
    json: { expectedVersion: recurring.version, ...changes },
  })
}

export function deleteRecurring(projectId: string, recurring: Pick<Recurring, 'id' | 'version'>, idempotencyKey: string) {
  return requestJson<unknown>(projectPath(projectId, `recurring-tasks/${encodeURIComponent(recurring.id)}`), {
    method: 'DELETE',
    idempotencyKey,
    json: { expectedVersion: recurring.version },
  })
}

// Project templates

export type ProjectTemplate = {
  id: string
  organizationId: string
  name: string
  description: string
  contains: { statuses: string[]; types: string[]; labels: number; channels: string[]; settings: number; taskTemplates: number }
  sourceProjectId: string | null
  sourceProjectName: string | null
  createdByName: string | null
  archived: boolean
  canEdit: boolean
  version: number
}

export function listProjectTemplates(organizationId: string, includeArchived = false) {
  return requestJson<{ templates: ProjectTemplate[] }>(
    `/api/v1/organizations/${encodeURIComponent(organizationId)}/project-templates${includeArchived ? '?includeArchived=true' : ''}`,
  )
}

export function saveAsProjectTemplate(projectId: string, input: { name: string; description: string }, idempotencyKey: string) {
  return requestJson<ProjectTemplate>(projectPath(projectId, 'save-as-template'), { method: 'POST', idempotencyKey, json: input })
}

export function updateProjectTemplate(
  template: Pick<ProjectTemplate, 'id' | 'organizationId' | 'version'>,
  changes: { name?: string; description?: string; archived?: boolean },
  idempotencyKey: string,
) {
  return requestJson<ProjectTemplate>(
    `/api/v1/organizations/${encodeURIComponent(template.organizationId)}/project-templates/${encodeURIComponent(template.id)}`,
    { method: 'PATCH', idempotencyKey, json: { expectedVersion: template.version, ...changes } },
  )
}

// Imports and export

export type ImportSource = 'markdown' | 'csv' | 'github'
export type ImportStatus = 'fetching' | 'draft' | 'queued' | 'running' | 'completed' | 'failed' | 'canceled'

export type ImportOptions = {
  headingsAsMilestones: boolean
  createLabels: boolean
  createMilestones: boolean
  githubState?: 'open' | 'all'
  excludedRows?: number[]
}

export type ImportPreviewRow = {
  index: number
  externalId: string | null
  title: string
  status: string | null
  type: string | null
  priority: string
  labels: string[]
  assignee: string | null
  milestone: string | null
  startDate: string | null
  dueDate: string | null
  estimate: number | null
  parent: number | null
  skip: string | null
  notes: string[]
}

export type ImportPreview = {
  total: number
  willCreate: number
  alreadyImported: number
  withoutTitle: number
  leftOut: number
  newLabels: string[]
  newMilestones: string[]
  unmatched: Record<string, Record<string, number>>
  rows: ImportPreviewRow[]
  rowsShown: number
}

export type TaskImport = {
  id: string
  source: ImportSource
  sourceName: string
  repositoryId: string | null
  repositoryName: string | null
  status: ImportStatus
  rowCount: number
  processedCount: number
  createdCount: number
  skippedCount: number
  error: string | null
  createdByName: string | null
  version: number
  createdAt: string
  finishedAt: string | null
  options: ImportOptions
  mapping: Record<string, string>
  columns?: string[]
  fields: string[]
  preview?: ImportPreview
  href: string
}

export type ImportDraft = {
  source: ImportSource
  text?: string
  fileName?: string
  repositoryId?: string
  options?: Partial<ImportOptions>
}

export function listImports(projectId: string) {
  return requestJson<{ imports: TaskImport[]; canImport: boolean }>(projectPath(projectId, 'imports'))
}

export function getImport(projectId: string, importId: string) {
  return requestJson<TaskImport>(projectPath(projectId, `imports/${encodeURIComponent(importId)}`))
}

export function createImport(projectId: string, draft: ImportDraft, idempotencyKey: string) {
  return requestJson<TaskImport>(projectPath(projectId, 'imports'), { method: 'POST', idempotencyKey, json: draft })
}

export function updateImport(
  projectId: string,
  taskImport: Pick<TaskImport, 'id' | 'version'>,
  changes: { mapping?: Record<string, string>; options?: ImportOptions; cancel?: boolean },
  idempotencyKey: string,
) {
  return requestJson<TaskImport>(projectPath(projectId, `imports/${encodeURIComponent(taskImport.id)}`), {
    method: 'PATCH',
    idempotencyKey,
    json: { expectedVersion: taskImport.version, ...changes },
  })
}

export function commitImport(projectId: string, taskImport: Pick<TaskImport, 'id' | 'version'>, idempotencyKey: string) {
  return requestJson<TaskImport>(projectPath(projectId, `imports/${encodeURIComponent(taskImport.id)}/commit`), {
    method: 'POST',
    idempotencyKey,
    json: { expectedVersion: taskImport.version },
  })
}

export function exportHref(projectId: string, format: 'json' | 'csv', includeArchived = false) {
  return apiUrl(projectPath(projectId, `export?format=${format}${includeArchived ? '&includeArchived=true' : ''}`))
}
