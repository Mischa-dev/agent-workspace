import { requestJson } from '../api/client'
import { normalizeCard, normalizeWorkItem, normalizeWorkflow } from './api'
import type { BulkChanges, Milestone, ProjectLabel, TaskRelationKind, WorkflowUpdate } from './types'
import { actorRefs, boolean, list, milestone, projectLabel, record } from './wire'

function projectPath(projectId: string, path: string) {
  return `/api/v1/projects/${encodeURIComponent(projectId)}/${path}`
}

function taskPath(projectId: string, workItemId: string, command: string) {
  return projectPath(projectId, `work-items/${encodeURIComponent(workItemId)}/${command}`)
}

export async function bulkUpdateWorkItems(input: {
  projectId: string
  items: Array<{ id: string; expectedVersion: number }>
  changes: BulkChanges
  idempotencyKey: string
}) {
  const payload = record(
    await requestJson<unknown>(projectPath(input.projectId, 'work-items/bulk'), {
      method: 'POST',
      idempotencyKey: input.idempotencyKey,
      json: { items: input.items, changes: input.changes },
    }),
  )
  return { items: list(payload.items).map(normalizeCard) }
}

export async function listLabels(projectId: string, includeArchived = false): Promise<ProjectLabel[]> {
  const query = includeArchived ? '?includeArchived=true' : ''
  const payload = record(await requestJson<unknown>(projectPath(projectId, `labels${query}`)))
  return list(payload.labels).map(projectLabel)
}

export async function createLabel(input: {
  projectId: string
  name: string
  color?: string
  description?: string
  idempotencyKey: string
}) {
  const { projectId, idempotencyKey, ...json } = input
  return projectLabel(
    await requestJson<unknown>(projectPath(projectId, 'labels'), { method: 'POST', idempotencyKey, json }),
  )
}

export async function updateLabel(input: {
  projectId: string
  labelId: string
  expectedVersion: number
  name?: string
  color?: string
  description?: string
  archived?: boolean
  idempotencyKey: string
}) {
  const { projectId, labelId, idempotencyKey, ...json } = input
  return projectLabel(
    await requestJson<unknown>(projectPath(projectId, `labels/${encodeURIComponent(labelId)}`), {
      method: 'PATCH',
      idempotencyKey,
      json,
    }),
  )
}

export async function listMilestones(projectId: string, includeArchived = false): Promise<{ milestones: Milestone[]; canEdit: boolean }> {
  const query = includeArchived ? '?includeArchived=true' : ''
  const payload = record(await requestJson<unknown>(projectPath(projectId, `milestones${query}`)))
  return { milestones: list(payload.milestones).map(milestone), canEdit: boolean(payload.canEdit) }
}

export async function createMilestone(input: {
  projectId: string
  name: string
  description?: string
  targetDate?: string | null
  idempotencyKey: string
}) {
  const { projectId, idempotencyKey, ...json } = input
  return milestone(
    await requestJson<unknown>(projectPath(projectId, 'milestones'), { method: 'POST', idempotencyKey, json }),
  )
}

export async function updateMilestone(input: {
  projectId: string
  milestoneId: string
  expectedVersion: number
  name?: string
  description?: string
  targetDate?: string | null
  archived?: boolean
  idempotencyKey: string
}) {
  const { projectId, milestoneId, idempotencyKey, ...json } = input
  return milestone(
    await requestJson<unknown>(projectPath(projectId, `milestones/${encodeURIComponent(milestoneId)}`), {
      method: 'PATCH',
      idempotencyKey,
      json,
    }),
  )
}

export async function setDependencies(input: {
  projectId: string
  workItemId: string
  expectedVersion: number
  blockedBy?: string[]
  blocks?: string[]
  remove?: boolean
  idempotencyKey: string
}) {
  const { projectId, workItemId, idempotencyKey, ...json } = input
  return normalizeWorkItem(
    await requestJson<unknown>(taskPath(projectId, workItemId, 'dependencies'), { method: 'POST', idempotencyKey, json }),
  )
}

export async function setRelation(input: {
  projectId: string
  workItemId: string
  expectedVersion: number
  kind: Exclude<TaskRelationKind, 'duplicated_by'>
  target: string
  remove?: boolean
  close?: boolean
  idempotencyKey: string
}) {
  const { projectId, workItemId, idempotencyKey, ...json } = input
  return normalizeWorkItem(
    await requestJson<unknown>(taskPath(projectId, workItemId, 'relations'), { method: 'POST', idempotencyKey, json }),
  )
}

export async function setWatching(input: { projectId: string; workItemId: string; watching: boolean; idempotencyKey: string }) {
  const payload = record(
    await requestJson<unknown>(taskPath(input.projectId, input.workItemId, 'watch'), {
      method: 'POST',
      idempotencyKey: input.idempotencyKey,
      json: { watching: input.watching },
    }),
  )
  return { watching: boolean(payload.watching), watchers: actorRefs(payload.watchers) }
}

export async function releaseClaim(input: { projectId: string; workItemId: string; idempotencyKey: string }) {
  return normalizeWorkItem(
    await requestJson<unknown>(taskPath(input.projectId, input.workItemId, 'release-claim'), {
      method: 'POST',
      idempotencyKey: input.idempotencyKey,
      json: {},
    }),
  )
}

export async function saveWorkflow(input: { projectId: string; workflow: WorkflowUpdate; idempotencyKey: string }) {
  return normalizeWorkflow(
    await requestJson<unknown>(projectPath(input.projectId, 'work-item-workflow'), {
      method: 'PUT',
      idempotencyKey: input.idempotencyKey,
      json: input.workflow,
    }),
  )
}
