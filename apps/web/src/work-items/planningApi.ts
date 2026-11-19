import { requestJson } from '../api/client'
import { callOp } from '../api/ops'
import { getWorkItem, normalizeCard, normalizeWorkflow } from './api'
import type { BulkChanges, Milestone, ProjectLabel, TaskRelationKind, WorkflowUpdate } from './types'
import { boolean, list, milestone, projectLabel, record } from './wire'

function projectPath(projectId: string, path: string) {
  return `/api/v1/projects/${encodeURIComponent(projectId)}/${path}`
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
  const { projectId, idempotencyKey, ...fields } = input
  const { label } = await callOp('label_create', { projectId, ...fields }, { requestId: idempotencyKey })
  return projectLabel(label)
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
  const { projectId, labelId, idempotencyKey, expectedVersion, ...fields } = input
  const { label } = await callOp('label_update', { projectId, label: labelId, ...fields }, { requestId: idempotencyKey, expectedVersion })
  return projectLabel(label)
}

export async function listMilestones(projectId: string, includeArchived = false): Promise<{ milestones: Milestone[]; canEdit: boolean }> {
  const query = includeArchived ? '?includeArchived=true' : ''
  const payload = record(await requestJson<unknown>(projectPath(projectId, `milestones${query}`)))
  return { milestones: list(payload.milestones).map(milestone), canEdit: boolean(payload.canEdit) }
}

/** A milestone's target date as the milestone ops take it: "" clears it. */
const targetDate = (value: string | null | undefined) => (value === null ? '' : value)

export async function createMilestone(input: {
  projectId: string
  name: string
  description?: string
  targetDate?: string | null
  idempotencyKey: string
}) {
  const { projectId, idempotencyKey, ...fields } = input
  return milestone(
    await callOp(
      'milestone_create',
      { projectId, ...fields, targetDate: targetDate(fields.targetDate) },
      { requestId: idempotencyKey },
    ),
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
  const { projectId, milestoneId, idempotencyKey, expectedVersion, ...fields } = input
  return milestone(
    await callOp(
      'milestone_update',
      { projectId, milestoneId, ...fields, targetDate: targetDate(fields.targetDate) },
      { requestId: idempotencyKey, expectedVersion },
    ),
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
  const { projectId, workItemId, idempotencyKey, expectedVersion, ...link } = input
  await callOp('task_link', { taskId: workItemId, ...link }, { requestId: idempotencyKey, expectedVersion })
  return getWorkItem(projectId, workItemId)
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
  const { projectId, workItemId, idempotencyKey, expectedVersion, target, ...link } = input
  await callOp('task_link', { taskId: workItemId, otherTaskId: target, ...link }, { requestId: idempotencyKey, expectedVersion })
  return getWorkItem(projectId, workItemId)
}

export async function setWatching(input: { projectId: string; workItemId: string; watching: boolean; idempotencyKey: string }) {
  await callOp('task_watch_set', { taskId: input.workItemId, watching: input.watching }, { requestId: input.idempotencyKey })
  const task = await getWorkItem(input.projectId, input.workItemId)
  return { watching: task.watching, watchers: task.watchers }
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
